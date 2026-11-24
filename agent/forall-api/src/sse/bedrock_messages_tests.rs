//! End-to-end tests for the Bedrock event-stream transport: hand-built binary
//! frames (valid prelude/message CRCs included) are driven all the way through
//! the shared Messages translator to [`ResponseEvent`]s.

use super::*;
use crate::event_stream::test_support::event_frame;
use crate::event_stream::test_support::exception_frame;
use crate::event_stream::test_support::frame;
use crate::event_stream::test_support::string_header;
use assert_matches::assert_matches;
use forall_protocol::models::ContentItem;
use forall_protocol::models::ResponseItem;
use forall_protocol::protocol::TokenUsage;
use pretty_assertions::assert_eq;
use serde_json::json;

fn idle_timeout() -> Duration {
    Duration::from_millis(1000)
}

/// Wraps one Anthropic chunk the way Bedrock does: base64 inside a `bytes`
/// property, with the `p` padding field Bedrock also emits.
fn chunk_frame(chunk: &Value) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(chunk.to_string());
    let payload = json!({"bytes": encoded, "p": "abcdefghij"}).to_string();
    let event_type = chunk
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    event_frame(event_type, payload.as_bytes())
}

/// Feeds `bytes` (already framed) through the decoder + translator and collects
/// everything emitted.
async fn run_frames(bytes: Vec<Vec<u8>>) -> Vec<Result<ResponseEvent, ApiError>> {
    run_frames_with_freeform(bytes, Vec::new()).await
}

async fn run_frames_with_freeform(
    bytes: Vec<Vec<u8>>,
    freeform_tool_names: Vec<String>,
) -> Vec<Result<ResponseEvent, ApiError>> {
    let stream = futures::stream::iter(
        bytes
            .into_iter()
            .map(|chunk| Ok::<bytes::Bytes, TransportError>(bytes::Bytes::from(chunk))),
    );
    let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
    tokio::spawn(process_bedrock_event_stream(
        Box::pin(stream),
        tx,
        idle_timeout(),
        freeform_tool_names,
    ));

    let mut out = Vec::new();
    while let Some(event) = rx.recv().await {
        out.push(event);
    }
    out
}

fn message_start(id: &str, usage: Value) -> Value {
    json!({
        "type": "message_start",
        "message": {
            "id": id,
            "type": "message",
            "role": "assistant",
            "model": "anthropic.claude-opus-4-6",
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": usage
        }
    })
}

fn text_turn_chunks() -> Vec<Value> {
    vec![
        message_start(
            "msg_bedrock_1",
            json!({"input_tokens": 12, "output_tokens": 1}),
        ),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": ""}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "Hel"}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "text_delta", "text": "lo"}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 9}
        }),
        json!({"type": "message_stop"}),
    ]
}

#[tokio::test]
async fn text_turn_over_event_stream_frames_completes() {
    let events = run_frames(text_turn_chunks().iter().map(chunk_frame).collect()).await;

    assert_eq!(events.len(), 6);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { role, .. }))
            if role == "assistant"
    );
    assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Hel");
    assert_matches!(&events[3], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "lo");
    assert_matches!(
        &events[4],
        Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
            if content == &vec![ContentItem::OutputText { text: "Hello".to_string() }]
    );
    match &events[5] {
        Ok(ResponseEvent::Completed {
            response_id,
            token_usage,
            end_turn,
        }) => {
            assert_eq!(response_id, "msg_bedrock_1");
            // Usage assembly is the shared translator's: `message_start`
            // input plus the cumulative `message_delta` output.
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
async fn a_turn_split_across_arbitrary_byte_chunks_completes() {
    // Every frame concatenated, then re-sliced into 7-byte transport chunks:
    // frame boundaries land mid-prelude, mid-header, and mid-payload.
    let mut all = Vec::new();
    for chunk in text_turn_chunks() {
        all.extend_from_slice(&chunk_frame(&chunk));
    }
    let transport_chunks: Vec<Vec<u8>> = all.chunks(7).map(<[u8]>::to_vec).collect();
    assert!(transport_chunks.len() > 10, "the split must be non-trivial");

    let events = run_frames(transport_chunks).await;

    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            end_turn: Some(true),
            ..
        }))
    );
}

#[tokio::test]
async fn several_frames_in_one_transport_chunk_complete() {
    let mut all = Vec::new();
    for chunk in text_turn_chunks() {
        all.extend_from_slice(&chunk_frame(&chunk));
    }

    let events = run_frames(vec![all]).await;

    assert_eq!(events.len(), 6);
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            end_turn: Some(true),
            ..
        }))
    );
}

#[tokio::test]
async fn tool_use_turn_over_event_stream_frames_emits_function_call() {
    let chunks = [
        message_start(
            "msg_bedrock_2",
            json!({"input_tokens": 8, "output_tokens": 0}),
        ),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "shell", "input": {}}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":"}
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "\"ls\"}"}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 15}
        }),
        json!({"type": "message_stop"}),
    ];

    let events = run_frames(chunks.iter().map(chunk_frame).collect()).await;

    assert_matches!(
        &events[1],
        Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall { name, call_id, .. }))
            if name == "shell" && call_id == "toolu_1"
    );
    let done = events
        .iter()
        .find_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(item)) => Some(item),
            _ => None,
        })
        .expect("an OutputItemDone for the tool call");
    match done {
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            assert_eq!(name, "shell");
            assert_eq!(call_id, "toolu_1");
            let parsed: Value =
                serde_json::from_str(arguments).expect("arguments should be a JSON string");
            assert_eq!(parsed, json!({"cmd": "ls"}));
        }
        other => panic!("expected a FunctionCall, got {other:?}"),
    }
    assert_matches!(
        events.last(),
        Some(Ok(ResponseEvent::Completed {
            end_turn: Some(false),
            ..
        }))
    );
}

#[tokio::test]
async fn freeform_tool_use_over_event_stream_frames_is_a_custom_tool_call() {
    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch";
    let arguments = json!({"input": patch}).to_string();
    let chunks = [
        message_start("msg_bedrock_ff", json!({"input_tokens": 5})),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "toolu_ff",
                "name": "apply_patch",
                "input": {}
            }
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": arguments}
        }),
        json!({"type": "content_block_stop", "index": 0}),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 4}
        }),
        json!({"type": "message_stop"}),
    ];

    let events = run_frames_with_freeform(
        chunks.iter().map(chunk_frame).collect(),
        vec!["apply_patch".to_string()],
    )
    .await;

    let done = events
        .iter()
        .find_map(|event| match event {
            Ok(ResponseEvent::OutputItemDone(item)) => Some(item),
            _ => None,
        })
        .expect("an OutputItemDone");
    match done {
        ResponseItem::CustomToolCall { name, input, .. } => {
            assert_eq!(name, "apply_patch");
            assert_eq!(input, patch);
        }
        other => panic!("freeform tools must not degrade to FunctionCall, got {other:?}"),
    }
}

#[tokio::test]
async fn cache_usage_fields_are_assembled_across_chunks() {
    let chunks = [
        message_start(
            "msg_bedrock_usage",
            json!({
                "input_tokens": 100,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 300,
                "output_tokens": 1
            }),
        ),
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn"},
            "usage": {"output_tokens": 50}
        }),
        json!({"type": "message_stop"}),
    ];

    let events = run_frames(chunks.iter().map(chunk_frame).collect()).await;

    assert_eq!(events.len(), 2);
    match &events[1] {
        Ok(ResponseEvent::Completed { token_usage, .. }) => {
            assert_eq!(
                token_usage,
                &Some(TokenUsage {
                    // 100 uncached + 20 cache-write + 300 cache-read.
                    input_tokens: 420,
                    cached_input_tokens: 300,
                    output_tokens: 50,
                    reasoning_output_tokens: 0,
                    total_tokens: 470,
                })
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[tokio::test]
async fn a_throttling_exception_mid_stream_is_retryable() {
    let mut frames: Vec<Vec<u8>> = text_turn_chunks()[..3].iter().map(chunk_frame).collect();
    frames.push(exception_frame(
        "throttlingException",
        br#"{"message":"Too many requests, please wait before trying again."}"#,
    ));
    // Frames after the exception must never be translated.
    frames.push(chunk_frame(&json!({"type": "message_stop"})));

    let events = run_frames(frames).await;

    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    match events.last() {
        Some(Err(ApiError::Retryable { message, delay })) => {
            assert_eq!(
                message,
                "Too many requests, please wait before trying again."
            );
            assert_eq!(*delay, None);
        }
        other => panic!("expected a Retryable error, got {other:?}"),
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Ok(ResponseEvent::Completed { .. }))),
        "a throttled stream must not report a completed turn"
    );
}

#[tokio::test]
async fn a_throttling_exception_honors_a_supplied_retry_delay() {
    let events = run_frames(vec![exception_frame(
        "throttlingException",
        br#"{"message":"slow down","retryAfterSeconds":12}"#,
    )])
    .await;

    assert_matches!(
        &events[0],
        Err(ApiError::Retryable { delay: Some(delay), .. }) if *delay == Duration::from_secs(12)
    );
}

#[tokio::test]
async fn a_validation_exception_is_an_invalid_request() {
    let events = run_frames(vec![exception_frame(
        "validationException",
        br#"{"message":"max_tokens must be at least 1"}"#,
    )])
    .await;

    assert_matches!(
        &events[0],
        Err(ApiError::InvalidRequest { message }) if message == "max_tokens must be at least 1"
    );
}

#[tokio::test]
async fn a_model_stream_error_exception_is_retryable() {
    let events = run_frames(vec![exception_frame(
        "modelStreamErrorException",
        br#"{"message":"upstream model failed"}"#,
    )])
    .await;

    assert_matches!(&events[0], Err(ApiError::Retryable { .. }));
}

#[tokio::test]
async fn an_expired_credential_exception_maps_to_http_unauthorized() {
    // Must surface as the 401 transport shape so core's retry loop refreshes
    // the credential instead of failing the turn.
    let events = run_frames(vec![exception_frame(
        "expiredTokenException",
        br#"{"message":"The security token included in the request is expired"}"#,
    )])
    .await;

    assert_matches!(
        &events[0],
        Err(ApiError::Transport(TransportError::Http { status, .. }))
            if *status == StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_error_frame_without_a_payload_message_still_reports_its_kind() {
    let mut headers = string_header(":message-type", "error");
    headers.extend_from_slice(&string_header(":error-code", "InternalServerException"));
    headers.extend_from_slice(&string_header(":error-message", "internal failure"));

    let events = run_frames(vec![frame(&headers, b"")]).await;

    assert_matches!(
        &events[0],
        Err(ApiError::Retryable { message, .. }) if message == "internal failure"
    );
}

#[tokio::test]
async fn a_stream_that_ends_without_message_stop_is_an_error() {
    let frames = text_turn_chunks()[..5].iter().map(chunk_frame).collect();

    let events = run_frames(frames).await;

    match events.last() {
        Some(Err(ApiError::Stream(message))) => {
            assert_eq!(message, "stream closed before response.completed");
        }
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn a_stream_that_ends_mid_frame_reports_truncation() {
    let complete = chunk_frame(&message_start("msg_trunc", json!({"input_tokens": 1})));
    let partial = chunk_frame(&json!({"type": "message_stop"}));
    let truncated = partial[..partial.len() - 4].to_vec();

    let events = run_frames(vec![complete, truncated]).await;

    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    match events.last() {
        Some(Err(ApiError::Stream(message))) => {
            assert_eq!(message, "bedrock event stream closed mid-frame");
        }
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn a_corrupt_frame_fails_the_stream_rather_than_being_trusted() {
    let mut corrupt = chunk_frame(&json!({"type": "message_stop"}));
    let last = corrupt.len() - 5;
    corrupt[last] ^= 0xFF;

    let events = run_frames(vec![
        chunk_frame(&message_start("msg_crc", json!({"input_tokens": 1}))),
        corrupt,
    ])
    .await;

    match events.last() {
        Some(Err(ApiError::Stream(message))) => {
            assert!(
                message.contains("CRC32 mismatch"),
                "error should name the framing failure: {message}"
            );
        }
        other => panic!("unexpected final event: {other:?}"),
    }
}

#[tokio::test]
async fn an_unreadable_event_payload_is_skipped_not_fatal() {
    // A frame whose payload is neither a wrapped nor a bare chunk must not end
    // the turn: the turn's own frames still complete it.
    let mut frames = vec![
        chunk_frame(&message_start("msg_skip", json!({"input_tokens": 1}))),
        event_frame("chunk", b"not json at all"),
        event_frame("chunk", br#"{"bytes":"!!!not-base64!!!"}"#),
        event_frame("chunk", br#"{"bytes": 7}"#),
    ];
    frames.push(chunk_frame(&json!({"type": "message_stop"})));

    let events = run_frames(frames).await;

    assert_eq!(events.len(), 2);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(&events[1], Ok(ResponseEvent::Completed { .. }));
}

#[tokio::test]
async fn a_bare_unwrapped_chunk_payload_is_accepted() {
    let events = run_frames(vec![
        event_frame(
            "message_start",
            message_start("msg_bare", json!({"input_tokens": 3}))
                .to_string()
                .as_bytes(),
        ),
        event_frame("message_stop", br#"{"type":"message_stop"}"#),
    ])
    .await;

    assert_eq!(events.len(), 2);
    assert_matches!(&events[0], Ok(ResponseEvent::Created));
    assert_matches!(
        &events[1],
        Ok(ResponseEvent::Completed { response_id, .. }) if response_id == "msg_bare"
    );
}

/// Chunk tracing is structural: it must name the event type and nothing that
/// could carry conversation content, and it must not depend on the chunk being
/// well-formed (the driver owns that judgement).
#[test]
fn chunk_type_names_the_discriminant_only() {
    assert_eq!(
        chunk_type(&json!({
            "type": "content_block_delta",
            "delta": {"type": "text_delta", "text": "secret"},
        })),
        "content_block_delta"
    );
    assert_eq!(chunk_type(&json!({"delta": {"text": "secret"}})), "?");
    assert_eq!(chunk_type(&json!({"type": 7})), "?");
    assert_eq!(chunk_type(&json!("content_block_delta")), "?");
    // A `type` that carries content rather than naming an event is bounded away,
    // not passed through; see `sse::event_trace::structural_label`.
    assert_eq!(
        chunk_type(&json!({"type": "content_block_delta with secret text"})),
        "<invalid>"
    );
}

#[tokio::test]
async fn spawn_bedrock_messages_stream_reports_the_amzn_request_id() {
    let mut headers = HeaderMap::new();
    headers.insert(
        AMZN_REQUEST_ID_HEADER,
        http::HeaderValue::from_static("11111111-2222-3333-4444-555555555555"),
    );
    let mut body = Vec::new();
    for chunk in [
        message_start("msg_hdr", json!({"input_tokens": 1})),
        json!({"type": "message_stop"}),
    ] {
        body.extend_from_slice(&chunk_frame(&chunk));
    }
    let bytes = futures::stream::iter(vec![Ok(bytes::Bytes::from(body))]);
    let stream_response = StreamResponse {
        status: StatusCode::OK,
        headers,
        bytes: Box::pin(bytes),
    };

    let mut stream = spawn_bedrock_messages_stream(
        stream_response,
        idle_timeout(),
        /*telemetry*/ None,
        /*freeform_tool_names*/ Vec::new(),
    );

    assert_eq!(
        stream.upstream_request_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    let mut events = Vec::new();
    while let Some(event) = stream.rx_event.recv().await {
        events.push(event.expect("expected ok event"));
    }
    assert_eq!(events.len(), 2);
    assert_matches!(&events[1], ResponseEvent::Completed { .. });
}

// ---------------------------------------------------------------------------
// HTTP-level error mapping
// ---------------------------------------------------------------------------

fn http_error(status: u16, error_type: Option<&str>, body: &str) -> ApiError {
    let mut headers = HeaderMap::new();
    if let Some(error_type) = error_type {
        headers.insert(
            AMZN_ERROR_TYPE_HEADER,
            http::HeaderValue::from_str(error_type).expect("valid header value"),
        );
    }
    ApiError::Transport(TransportError::Http {
        status: StatusCode::from_u16(status).expect("valid status"),
        url: None,
        headers: Some(headers),
        body: Some(body.to_string()),
    })
}

#[test]
fn http_throttling_maps_to_retryable_with_retry_after() {
    let mut headers = HeaderMap::new();
    headers.insert(
        AMZN_ERROR_TYPE_HEADER,
        http::HeaderValue::from_static("ThrottlingException:http://internal.amazon.com/coral/x"),
    );
    headers.insert(
        http::header::RETRY_AFTER,
        http::HeaderValue::from_static("7"),
    );
    let error = ApiError::Transport(TransportError::Http {
        status: StatusCode::TOO_MANY_REQUESTS,
        url: None,
        headers: Some(headers),
        body: Some(r#"{"message":"Too many tokens"}"#.to_string()),
    });

    match map_bedrock_http_error(error) {
        ApiError::Retryable { message, delay } => {
            assert_eq!(message, "Too many tokens");
            assert_eq!(delay, Some(Duration::from_secs(7)));
        }
        other => panic!("unexpected mapping: {other:?}"),
    }
}

#[test]
fn http_validation_error_maps_to_invalid_request() {
    match map_bedrock_http_error(http_error(
        400,
        Some("ValidationException"),
        r#"{"message":"extraneous key [stream] is not permitted"}"#,
    )) {
        ApiError::InvalidRequest { message } => {
            assert_eq!(message, "extraneous key [stream] is not permitted");
        }
        other => panic!("unexpected mapping: {other:?}"),
    }
}

#[test]
fn http_access_denied_maps_to_invalid_request() {
    // An IAM policy denial is not fixable by refreshing a credential, so it
    // must not be routed into the unauthorized-retry loop.
    match map_bedrock_http_error(http_error(
        403,
        Some("AccessDeniedException"),
        r#"{"message":"not authorized to invoke this model"}"#,
    )) {
        ApiError::InvalidRequest { message } => {
            assert_eq!(message, "not authorized to invoke this model");
        }
        other => panic!("unexpected mapping: {other:?}"),
    }
}

#[test]
fn http_unrecognized_client_maps_to_http_unauthorized() {
    assert_matches!(
        map_bedrock_http_error(http_error(
            403,
            Some("UnrecognizedClientException"),
            r#"{"message":"The security token included in the request is invalid"}"#,
        )),
        ApiError::Transport(TransportError::Http { status, .. })
            if status == StatusCode::UNAUTHORIZED
    );
}

#[test]
fn http_error_type_is_read_from_the_body_when_the_header_is_absent() {
    match map_bedrock_http_error(http_error(
        500,
        /*error_type*/ None,
        r#"{"__type":"com.amazon.coral.service#InternalServerException","message":"boom"}"#,
    )) {
        ApiError::Retryable { message, .. } => assert_eq!(message, "boom"),
        other => panic!("unexpected mapping: {other:?}"),
    }
}

#[test]
fn http_unauthorized_passes_through_for_core_retry_loop() {
    assert_matches!(
        map_bedrock_http_error(http_error(
            401,
            Some("ValidationException"),
            r#"{"message":"ignored"}"#,
        )),
        ApiError::Transport(TransportError::Http { status, .. })
            if status == StatusCode::UNAUTHORIZED
    );
}

#[test]
fn http_unrecognized_error_passes_through_unchanged() {
    assert_matches!(
        map_bedrock_http_error(http_error(
            502,
            Some("SomeFutureException"),
            "<html>gateway error</html>",
        )),
        ApiError::Transport(TransportError::Http { status, .. })
            if status == StatusCode::BAD_GATEWAY
    );
}

#[test]
fn non_transport_errors_pass_through_unchanged() {
    assert_matches!(
        map_bedrock_http_error(ApiError::ContextWindowExceeded),
        ApiError::ContextWindowExceeded
    );
}
