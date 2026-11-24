use super::*;
use assert_matches::assert_matches;
use futures::TryStreamExt;
use http::HeaderValue;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio_util::io::ReaderStream;

fn idle_timeout() -> Duration {
    Duration::from_millis(1000)
}

/// Feeds a recorded Gemini SSE transcript (one `GenerateContentResponse` per
/// event) through the state machine and returns every emitted event/error.
///
/// Gemini emits data-only SSE frames (no `event:` line), which is what this
/// reproduces.
async fn run_gemini_sse(chunks: Vec<serde_json::Value>) -> Vec<Result<ResponseEvent, ApiError>> {
    run_gemini_sse_with_freeform(chunks, Vec::new()).await
}

async fn run_gemini_sse_with_freeform(
    chunks: Vec<serde_json::Value>,
    freeform_tool_names: Vec<String>,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str(&format!("data: {chunk}\n\n"));
    }
    run_gemini_sse_bytes(body, freeform_tool_names).await
}

async fn run_gemini_sse_bytes(
    body: String,
    freeform_tool_names: Vec<String>,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let stream = ReaderStream::new(std::io::Cursor::new(body))
        .map_err(|err| TransportError::Network(err.to_string()));
    let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
    tokio::spawn(process_gemini_sse(
        Box::pin(stream),
        tx,
        idle_timeout(),
        /*telemetry*/ None,
        freeform_tool_names,
    ));

    let mut out = Vec::new();
    while let Some(event) = rx.recv().await {
        out.push(event);
    }
    out
}

/// A text chunk with no terminal `finishReason`.
fn text_chunk(response_id: &str, text: &str) -> serde_json::Value {
    json!({
        "responseId": response_id,
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": text}]},
            "index": 0
        }],
        "modelVersion": "gemini-2.5-pro"
    })
}

/// The terminal chunk: `finishReason` plus cumulative `usageMetadata`.
fn finish_chunk(
    response_id: &str,
    finish_reason: &str,
    usage: serde_json::Value,
) -> serde_json::Value {
    json!({
        "responseId": response_id,
        "candidates": [{"content": {"role": "model", "parts": []}, "finishReason": finish_reason}],
        "usageMetadata": usage
    })
}

fn function_call_chunk(
    response_id: &str,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    json!({
        "responseId": response_id,
        "candidates": [{
            "content": {"role": "model", "parts": [{"functionCall": {"name": name, "args": args}}]}
        }]
    })
}

fn decode_envelope(encrypted_content: &str) -> serde_json::Value {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encrypted_content)
        .expect("envelope should be valid base64");
    serde_json::from_slice(&decoded).expect("envelope should be JSON")
}

fn completed_end_turn(event: &Result<ResponseEvent, ApiError>) -> Option<bool> {
    match event {
        Ok(ResponseEvent::Completed { end_turn, .. }) => *end_turn,
        other => panic!("expected Completed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_only_turn_emits_triple_and_completed() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_1", "Hel"),
        text_chunk("resp_1", "lo"),
        finish_chunk(
            "resp_1",
            "STOP",
            json!({
                "promptTokenCount": 12,
                "candidatesTokenCount": 9,
                "totalTokenCount": 21
            }),
        ),
    ])
    .await;

    assert_eq!(events.len(), 6);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
            role,
            content,
            phase: None,
            ..
        })) if role == "assistant" && content.is_empty()
    );
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Hel");
    assert_matches!(&events[3], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "lo");
    assert_matches!(
        &events[4],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Message {
            role,
            content,
            phase: None,
            ..
        })) if role == "assistant"
            && content == &vec![ContentItem::OutputText { text: "Hello".to_string() }]
    );
    match &events[5] {
        Ok(ResponseEvent::Completed {
            response_id,
            token_usage,
            end_turn,
        }) => {
            assert_eq!(response_id, "resp_1");
            assert_eq!(
                token_usage,
                &Some(TokenUsage {
                    input_tokens: 12,
                    cached_input_tokens: 0,
                    output_tokens: 9,
                    reasoning_output_tokens: 0,
                    total_tokens: 21,
                })
            );
            assert_eq!(*end_turn, Some(true));
        }
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn text_in_the_terminal_chunk_is_flushed_before_completed() {
    let events = run_gemini_sse(vec![json!({
        "responseId": "resp_tail",
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "done"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
    })])
    .await;

    assert_eq!(events.len(), 5);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
    );
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "done");
    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
            if content == &vec![ContentItem::OutputText { text: "done".to_string() }]
    );
    assert_matches!(&events[4], Ok(ResponseEvent::Completed { .. }));
}

#[tokio::test]
async fn empty_text_parts_do_not_open_an_item() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_empty", ""),
        finish_chunk("resp_empty", "STOP", json!({"promptTokenCount": 3})),
    ])
    .await;

    // Only `Created` and `Completed`: an empty part would otherwise produce an
    // empty assistant message.
    assert_eq!(events.len(), 2);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(&events[1], Ok(ResponseEvent::Completed { .. }));
}

// ---------------------------------------------------------------------------
// Reasoning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thought_parts_build_a_signed_reasoning_envelope() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_2",
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "Let me think", "thought": true}]}
            }]
        }),
        json!({
            "responseId": "resp_2",
            "candidates": [{
                "content": {"role": "model", "parts": [{
                    "text": " harder",
                    "thought": true,
                    "thoughtSignature": "sig-abc"
                }]}
            }]
        }),
        text_chunk("resp_2", "Answer"),
        finish_chunk(
            "resp_2",
            "STOP",
            json!({
                "promptTokenCount": 5,
                "candidatesTokenCount": 4,
                "thoughtsTokenCount": 30,
                "totalTokenCount": 39
            }),
        ),
    ])
    .await;

    assert_eq!(events.len(), 9);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
            encrypted_content: None,
            content: None,
            ..
        }))
    );
    assert_matches!(
        &events[2],
        Ok(ResponseEvent::ReasoningContentDelta { delta, content_index: 0 })
            if delta == "Let me think"
    );
    assert_matches!(
        &events[3],
        Ok(ResponseEvent::ReasoningContentDelta { delta, content_index: 0 })
            if delta == " harder"
    );
    match &events[4] {
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
            content,
            encrypted_content,
            ..
        })) => {
            assert_eq!(
                content,
                &Some(vec![ReasoningItemContent::ReasoningText {
                    text: "Let me think harder".to_string(),
                }])
            );
            let envelope = decode_envelope(encrypted_content.as_deref().expect("signed envelope"));
            assert_eq!(
                envelope,
                json!({"text": "Let me think harder", "thoughtSignature": "sig-abc"})
            );
        }
        other => panic!("unexpected reasoning done event: {other:?}"),
    }
    // Reasoning closes when the first non-thought text part arrives.
    assert_matches!(
        &events[5],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
    );
    assert_matches!(&events[6], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Answer");
    assert_matches!(
        &events[7],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
    );
    match &events[8] {
        Ok(ResponseEvent::Completed { token_usage, .. }) => assert_eq!(
            token_usage,
            &Some(TokenUsage {
                input_tokens: 5,
                cached_input_tokens: 0,
                // candidatesTokenCount + thoughtsTokenCount.
                output_tokens: 34,
                reasoning_output_tokens: 30,
                total_tokens: 39,
            })
        ),
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn unsigned_thought_leaves_the_envelope_empty() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_unsigned",
            "candidates": [{
                "content": {"role": "model", "parts": [{"text": "musing", "thought": true}]}
            }]
        }),
        finish_chunk("resp_unsigned", "STOP", json!({"promptTokenCount": 1})),
    ])
    .await;

    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
            encrypted_content: None,
            ..
        }))
    );
}

// ---------------------------------------------------------------------------
// Function calls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn function_call_emits_an_atomic_triple_with_stringified_args() {
    let events = run_gemini_sse(vec![
        function_call_chunk("resp_3", "shell", json!({"command": ["ls", "/tmp"]})),
        finish_chunk(
            "resp_3",
            "STOP",
            json!({"promptTokenCount": 8, "candidatesTokenCount": 12}),
        ),
    ])
    .await;

    assert_eq!(events.len(), 5);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    let call_id = match &events[1] {
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        })) => {
            assert_eq!(name, "shell");
            assert_eq!(arguments, "");
            // Gemini issues no call ids, so one is synthesized from the
            // response id.
            assert_eq!(call_id, "resp_3-call-0");
            call_id.clone()
        }
        other => panic!("unexpected added event: {other:?}"),
    };
    assert_matches!(
        &events[2],
        Ok(ResponseEvent::ToolCallInputDelta { item_id, call_id: delta_call_id, delta })
            if item_id == &call_id
                && delta_call_id.as_deref() == Some(call_id.as_str())
                && delta == r#"{"command":["ls","/tmp"]}"#
    );
    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            name,
            arguments,
            ..
        })) if name == "shell" && arguments == r#"{"command":["ls","/tmp"]}"#
    );
    // A tool call means the turn is not over.
    assert_eq!(completed_end_turn(&events[4]), Some(false));
}

#[tokio::test]
async fn function_call_without_args_gets_an_empty_object() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_noargs",
            "candidates": [{
                "content": {"role": "model", "parts": [{"functionCall": {"name": "ping"}}]}
            }]
        }),
        finish_chunk("resp_noargs", "STOP", json!({"promptTokenCount": 1})),
    ])
    .await;

    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }))
            if arguments == "{}"
    );
}

#[tokio::test]
async fn server_supplied_call_id_is_preferred_over_the_synthesized_one() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_ids",
            "candidates": [{
                "content": {"role": "model", "parts": [{
                    "functionCall": {"id": "fc_server", "name": "shell", "args": {}}
                }]}
            }]
        }),
        finish_chunk("resp_ids", "STOP", json!({"promptTokenCount": 1})),
    ])
    .await;

    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall { call_id, .. }))
            if call_id == "fc_server"
    );
}

#[tokio::test]
async fn parallel_function_calls_across_chunks_emit_sequential_triples() {
    let events = run_gemini_sse(vec![
        function_call_chunk("resp_4", "first", json!({"a": 1})),
        function_call_chunk("resp_4", "second", json!({"b": 2})),
        finish_chunk("resp_4", "STOP", json!({"promptTokenCount": 4})),
    ])
    .await;

    assert_eq!(events.len(), 8);
    let names: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, .. })) => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["first", "second"]);
    // Synthesized ids stay unique within the response.
    let call_ids: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall { call_id, .. })) => {
                Some(call_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(call_ids, vec!["resp_4-call-0", "resp_4-call-1"]);
    assert_eq!(completed_end_turn(&events[7]), Some(false));
}

#[tokio::test]
async fn multi_part_chunk_closes_each_item_before_opening_the_next() {
    // One chunk carrying thought + text + functionCall, which is how Gemini
    // packs a turn that reasons, narrates, then calls a tool.
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_5",
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "thinking", "thought": true, "thoughtSignature": "sig-1"},
                    {"text": "I will list files."},
                    {"functionCall": {"name": "shell", "args": {"command": ["ls"]}}}
                ]}
            }]
        }),
        finish_chunk("resp_5", "STOP", json!({"promptTokenCount": 7})),
    ])
    .await;

    let kinds: Vec<&str> = events
        .iter()
        .map(|event| match event {
            Ok(ResponseEvent::Created) => "created",
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. })) => "added_reasoning",
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. })) => "added_message",
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall { .. })) => "added_call",
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })) => "done_reasoning",
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { .. })) => "done_message",
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { .. })) => "done_call",
            Ok(ResponseEvent::ReasoningContentDelta { .. }) => "reasoning_delta",
            Ok(ResponseEvent::OutputTextDelta(_)) => "text_delta",
            Ok(ResponseEvent::ToolCallInputDelta { .. }) => "tool_delta",
            Ok(ResponseEvent::Completed { .. }) => "completed",
            other => panic!("unexpected event: {other:?}"),
        })
        .collect();

    assert_eq!(
        kinds,
        vec![
            "created",
            "added_reasoning",
            "reasoning_delta",
            "done_reasoning",
            "added_message",
            "text_delta",
            "done_message",
            "added_call",
            "tool_delta",
            "done_call",
            "completed",
        ]
    );
}

#[tokio::test]
async fn freeform_tool_call_is_unwrapped_into_a_custom_tool_call() {
    let events = run_gemini_sse_with_freeform(
        vec![
            function_call_chunk(
                "resp_6",
                "apply_patch",
                json!({"input": "*** Begin Patch\n*** End Patch"}),
            ),
            finish_chunk("resp_6", "STOP", json!({"promptTokenCount": 2})),
        ],
        vec!["apply_patch".to_string()],
    )
    .await;

    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::CustomToolCall { name, input, .. }))
            if name == "apply_patch" && input.is_empty()
    );
    assert_matches!(
        &events[2],
        Ok(ResponseEvent::ToolCallInputDelta { delta, .. })
            if delta == "*** Begin Patch\n*** End Patch"
    );
    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall { name, input, .. }))
            if name == "apply_patch" && input == "*** Begin Patch\n*** End Patch"
    );
}

#[tokio::test]
async fn freeform_tool_call_off_schema_keeps_raw_arguments_as_the_payload() {
    let events = run_gemini_sse_with_freeform(
        vec![
            function_call_chunk("resp_7", "apply_patch", json!({"patch": "oops"})),
            finish_chunk("resp_7", "STOP", json!({"promptTokenCount": 2})),
        ],
        vec!["apply_patch".to_string()],
    )
    .await;

    assert_matches!(
        &events[3],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::CustomToolCall { input, .. }))
            if input == r#"{"patch":"oops"}"#
    );
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cached_tokens_are_reported_without_being_folded_into_the_prompt_count() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_8", "hi"),
        finish_chunk(
            "resp_8",
            "STOP",
            json!({
                "promptTokenCount": 1_000,
                "cachedContentTokenCount": 800,
                "candidatesTokenCount": 20,
                "thoughtsTokenCount": 5,
                "totalTokenCount": 1_025
            }),
        ),
    ])
    .await;

    match events.last().expect("completed") {
        Ok(ResponseEvent::Completed { token_usage, .. }) => assert_eq!(
            token_usage,
            &Some(TokenUsage {
                // promptTokenCount already includes the 800 cached tokens; they
                // are NOT added again (unlike the Anthropic Messages adapter).
                input_tokens: 1_000,
                cached_input_tokens: 800,
                output_tokens: 25,
                reasoning_output_tokens: 5,
                total_tokens: 1_025,
            })
        ),
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn cumulative_usage_is_assigned_not_accumulated() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_9",
            "candidates": [{"content": {"role": "model", "parts": [{"text": "a"}]}}],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 1}
        }),
        json!({
            "responseId": "resp_9",
            "candidates": [{"content": {"role": "model", "parts": [{"text": "b"}]}}],
            "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 2}
        }),
        finish_chunk(
            "resp_9",
            "STOP",
            json!({"promptTokenCount": 10, "candidatesTokenCount": 3}),
        ),
    ])
    .await;

    match events.last().expect("completed") {
        Ok(ResponseEvent::Completed { token_usage, .. }) => assert_eq!(
            token_usage,
            &Some(TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 3,
                reasoning_output_tokens: 0,
                // Synthesized: no totalTokenCount on the wire.
                total_tokens: 13,
            })
        ),
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn missing_usage_metadata_yields_no_token_usage() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_10", "hi"),
        json!({
            "responseId": "resp_10",
            "candidates": [{"content": {"role": "model", "parts": []}, "finishReason": "STOP"}]
        }),
    ])
    .await;

    match events.last().expect("completed") {
        Ok(ResponseEvent::Completed { token_usage, .. }) => assert_eq!(token_usage, &None),
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn synthesized_response_id_is_used_when_the_server_omits_one() {
    let events = run_gemini_sse(vec![json!({
        "candidates": [{
            "content": {"role": "model", "parts": [{"text": "hi"}]},
            "finishReason": "STOP"
        }]
    })])
    .await;

    match events.last().expect("completed") {
        Ok(ResponseEvent::Completed { response_id, .. }) => {
            assert!(
                response_id.starts_with("gemini-local-"),
                "expected a synthesized id, got {response_id}"
            );
        }
        other => panic!("unexpected final event: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// finishReason matrix
// ---------------------------------------------------------------------------

#[tokio::test]
async fn finish_reason_max_tokens_surfaces_truncation_instead_of_completing() {
    // Behavior change (PR #80 review): MAX_TOKENS used to emit Completed, so
    // core persisted a half-written answer as the finished turn. It now fails
    // the stream, matching how the Responses adapter treats
    // `response.incomplete`.
    let events = run_gemini_sse(vec![
        text_chunk("resp_11", "truncat"),
        finish_chunk("resp_11", "MAX_TOKENS", json!({"promptTokenCount": 1})),
    ])
    .await;

    // Partial text is still delivered before the error.
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(_)));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
        "truncation must not be reported as a completed turn"
    );
    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::Stream(message)) if message.contains("truncated")
    );
}

#[tokio::test]
async fn finish_reason_max_tokens_fails_even_with_a_tool_call() {
    // A tool call cut off by the token limit may have truncated arguments, so
    // this is if anything worse than truncated prose — never a normal turn end.
    let events = run_gemini_sse(vec![
        function_call_chunk("resp_12", "shell", json!({})),
        finish_chunk("resp_12", "MAX_TOKENS", json!({"promptTokenCount": 1})),
    ])
    .await;

    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::Stream(message)) if message.contains("truncated")
    );
}

#[tokio::test]
async fn finish_reason_safety_is_an_invalid_request_error() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_13", "partial"),
        finish_chunk("resp_13", "SAFETY", json!({"promptTokenCount": 1})),
    ])
    .await;

    // The partial text is still delivered, then the error replaces `Completed`
    // so the truncation is never mistaken for a finished turn.
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(_)));
    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::InvalidRequest { message }) if message.contains("SAFETY")
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
        "a blocked candidate must not produce Completed"
    );
}

#[tokio::test]
async fn content_policy_finish_reasons_all_map_to_invalid_request() {
    for reason in [
        "RECITATION",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "SPII",
        "IMAGE_SAFETY",
        "LANGUAGE",
        "MALFORMED_FUNCTION_CALL",
        "UNEXPECTED_TOOL_CALL",
    ] {
        let events = run_gemini_sse(vec![finish_chunk(
            "resp_policy",
            reason,
            json!({"promptTokenCount": 1}),
        )])
        .await;

        assert_matches!(
            events.last().expect("error"),
            Err(ApiError::InvalidRequest { message }) if message.contains(reason),
            "{reason} should map to InvalidRequest"
        );
    }
}

#[tokio::test]
async fn finish_reason_other_maps_to_a_retryable_stream_error() {
    let events = run_gemini_sse(vec![finish_chunk(
        "resp_14",
        "OTHER",
        json!({"promptTokenCount": 1}),
    )])
    .await;

    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::Stream(message)) if message.contains("OTHER")
    );
}

#[tokio::test]
async fn unknown_finish_reason_completes_with_no_end_turn() {
    let events = run_gemini_sse(vec![
        text_chunk("resp_15", "hi"),
        finish_chunk(
            "resp_15",
            "FINISH_REASON_UNSPECIFIED",
            json!({"promptTokenCount": 1}),
        ),
    ])
    .await;

    assert_eq!(completed_end_turn(events.last().expect("completed")), None);
}

// ---------------------------------------------------------------------------
// Termination and errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_without_a_finish_reason_reports_closed_before_completed() {
    let events = run_gemini_sse(vec![text_chunk("resp_16", "half a sentence")]).await;

    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::Stream(message)) if message == "stream closed before response.completed"
    );
}

#[tokio::test]
async fn empty_stream_reports_closed_before_completed() {
    let events = run_gemini_sse(Vec::new()).await;

    assert_eq!(events.len(), 1);
    assert_matches!(
        &events[0],
        Err(ApiError::Stream(message)) if message == "stream closed before response.completed"
    );
}

#[tokio::test]
async fn in_band_error_payload_terminates_the_stream() {
    let events = run_gemini_sse(vec![json!({
        "error": {"code": 429, "message": "quota exceeded", "status": "RESOURCE_EXHAUSTED"}
    })])
    .await;

    assert_eq!(events.len(), 1);
    assert_matches!(
        &events[0],
        Err(ApiError::Retryable { message, delay: None }) if message == "quota exceeded"
    );
}

#[tokio::test]
async fn blocked_prompt_feedback_is_an_invalid_request_error() {
    let events = run_gemini_sse(vec![json!({
        "promptFeedback": {"blockReason": "SAFETY"},
        "usageMetadata": {"promptTokenCount": 12}
    })])
    .await;

    assert_matches!(
        events.last().expect("error"),
        Err(ApiError::InvalidRequest { message }) if message.contains("SAFETY")
    );
}

#[tokio::test]
async fn unparseable_chunks_are_skipped() {
    let mut body = String::new();
    body.push_str("data: {not json}\n\n");
    body.push_str(&format!("data: {}\n\n", text_chunk("resp_17", "ok")));
    body.push_str(&format!(
        "data: {}\n\n",
        finish_chunk("resp_17", "STOP", json!({"promptTokenCount": 1}))
    ));

    let events = run_gemini_sse_bytes(body, Vec::new()).await;

    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "ok");
    assert_matches!(
        events.last().expect("completed"),
        Ok(ResponseEvent::Completed { .. })
    );
}

#[tokio::test]
async fn unknown_part_kinds_are_ignored() {
    let events = run_gemini_sse(vec![
        json!({
            "responseId": "resp_18",
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"inlineData": {"mimeType": "image/png", "data": "AAAA"}},
                    {"executableCode": {"language": "PYTHON", "code": "print(1)"}}
                ]}
            }]
        }),
        finish_chunk("resp_18", "STOP", json!({"promptTokenCount": 1})),
    ])
    .await;

    assert_eq!(events.len(), 2);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(&events[1], Ok(ResponseEvent::Completed { .. }));
}

// ---------------------------------------------------------------------------
// HTTP error mapping
// ---------------------------------------------------------------------------

fn http_error(status: StatusCode, body: &str, retry_after: Option<&str>) -> ApiError {
    let headers = retry_after.map(|value| {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::RETRY_AFTER,
            HeaderValue::from_str(value).expect("valid retry-after"),
        );
        headers
    });
    ApiError::Transport(TransportError::Http {
        status,
        url: None,
        headers,
        body: Some(body.to_string()),
    })
}

#[test]
fn http_429_maps_to_retryable_with_the_retry_after_delay() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}}"#,
        Some("30"),
    ));

    assert_matches!(
        mapped,
        ApiError::Retryable { message, delay } if message == "quota" && delay == Some(Duration::from_secs(30))
    );
}

#[test]
fn http_503_maps_to_retryable() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::SERVICE_UNAVAILABLE,
        r#"{"error":{"code":503,"message":"overloaded","status":"UNAVAILABLE"}}"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::Retryable { message, delay: None } if message == "overloaded"
    );
}

#[test]
fn http_400_maps_to_invalid_request() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::BAD_REQUEST,
        r#"{"error":{"code":400,"message":"bad schema","status":"INVALID_ARGUMENT"}}"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::InvalidRequest { message } if message == "bad schema"
    );
}

#[test]
fn http_403_preserves_the_provider_message_instead_of_becoming_a_401() {
    // Behavior change (PR #80 review): Google returns PERMISSION_DENIED/403 for
    // billing, API-not-enabled, project config and regional restrictions too. A
    // key refresh fixes none of those, and rewriting them to 401 replaced
    // Google's actionable message with a generic unauthenticated failure.
    let mapped = map_gemini_http_error(http_error(
        StatusCode::FORBIDDEN,
        r#"{"error":{"code":403,"message":"Gemini API has not been used in project 123 before or it is disabled","status":"PERMISSION_DENIED"}}"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::InvalidRequest { message } if message.contains("has not been used in project")
    );
}

#[test]
fn http_401_passes_through_untouched() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::UNAUTHORIZED,
        r#"{"error":{"code":401,"message":"no key","status":"UNAUTHENTICATED"}}"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::Transport(TransportError::Http { status, body: Some(body), .. })
            if status == StatusCode::UNAUTHORIZED && body.contains("UNAUTHENTICATED")
    );
}

#[test]
fn array_wrapped_error_body_is_parsed() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"[{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}}]"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::Retryable { message, delay: None } if message == "quota"
    );
}

#[test]
fn unrecognized_error_body_passes_through_unchanged() {
    let mapped = map_gemini_http_error(http_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "<html>gateway error</html>",
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::Transport(TransportError::Http { status, .. })
            if status == StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn unknown_status_string_keeps_the_original_transport_error() {
    // A well-formed body whose status we do not classify must not be flattened
    // into a bare stream error: the HTTP status is diagnostically load-bearing.
    let mapped = map_gemini_http_error(http_error(
        StatusCode::IM_A_TEAPOT,
        r#"{"error":{"code":418,"message":"teapot","status":"WEIRD"}}"#,
        None,
    ));

    assert_matches!(
        mapped,
        ApiError::Transport(TransportError::Http { status, .. })
            if status == StatusCode::IM_A_TEAPOT
    );
}

#[test]
fn non_transport_errors_pass_through_unchanged() {
    let mapped = map_gemini_http_error(ApiError::ContextWindowExceeded);
    assert_matches!(mapped, ApiError::ContextWindowExceeded);
}

#[test]
fn chunk_shape_names_structure_only() {
    let chunk: GenerateContentChunk = serde_json::from_value(json!({
        "responseId": "resp_1",
        "candidates": [{
            "content": {"parts": [
                {"text": "secret thinking", "thought": true, "thoughtSignature": "sig"},
                {"text": "secret answer"},
                {"functionCall": {"name": "shell", "args": {"command": "secret"}}},
            ]},
            "finishReason": "STOP",
        }],
        "usageMetadata": {"totalTokenCount": 7},
    }))
    .expect("chunk deserializes");

    let shape = chunk_shape(&chunk);
    assert_eq!(
        shape,
        "candidates=1 parts=[thought,text,function_call] finish_reason=STOP usage"
    );
    assert!(!shape.contains("secret"), "traced body content: {shape}");
}

#[test]
fn chunk_shape_covers_chunks_carrying_no_parts() {
    let shape = |value| chunk_shape(&serde_json::from_value(value).expect("chunk deserializes"));

    assert_eq!(shape(json!({"usageMetadata": {}})), "candidates=0 usage");
    assert_eq!(
        shape(json!({"promptFeedback": {"blockReason": "SAFETY"}})),
        "candidates=0 block_reason=SAFETY"
    );
    assert_eq!(
        shape(json!({"error": {"code": 429, "message": "secret detail"}})),
        "candidates=0 error"
    );
    // A candidate with no content at all, and parts naming no kind.
    assert_eq!(shape(json!({"candidates": [{}]})), "candidates=1 parts=[]");
    // Nothing constrains these two to be enum names on the wire, so a provider
    // can put content there. They are bounded, not trusted.
    assert_eq!(
        shape(json!({"candidates": [{"finishReason": "stopped: user asked about X"}]})),
        "candidates=1 parts=[] finish_reason=<invalid>"
    );
    assert_eq!(
        shape(json!({"promptFeedback": {"blockReason": "blocked: prompt mentioned X"}})),
        "candidates=0 block_reason=<invalid>"
    );
    assert_eq!(
        shape(json!({"candidates": [{"content": {"parts": [{"thoughtSignature": "sig"}, {}]}}]})),
        "candidates=1 parts=[thought_signature,?]"
    );
}
