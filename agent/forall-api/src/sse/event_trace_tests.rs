use super::*;
use crate::common::ResponseEvent;
use crate::error::ApiError;
use forall_client::TransportError;
use futures::TryStreamExt;
use serde_json::json;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::io::ReaderStream;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

fn sse(event: &str, data: &str) -> Event {
    Event {
        event: event.to_string(),
        data: data.to_string(),
        ..Default::default()
    }
}

/// The payload of this chunk is what must never reach a log: `secret` stands in
/// for generated text.
const TEXT_DELTA: &str =
    r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"secret"}}"#;

#[test]
fn sse_event_kind_prefers_the_wire_event_name() {
    let event = sse("content_block_delta", TEXT_DELTA);
    assert_eq!(sse_event_kind(&event), "content_block_delta");
}

#[test]
fn sse_event_kind_falls_back_to_the_payload_discriminant() {
    // Both spellings of "the stream named no event".
    for name in ["", DEFAULT_EVENT_NAME] {
        assert_eq!(
            sse_event_kind(&sse(name, TEXT_DELTA)),
            "content_block_delta"
        );
    }
    assert_eq!(
        sse_event_kind(&sse(
            "",
            r#"{"type":"response.output_text.delta","delta":"secret"}"#
        )),
        "response.output_text.delta"
    );
}

#[test]
fn sse_event_kind_is_unnamed_when_nothing_names_the_event() {
    // Gemini: no `event:` field and no discriminant in the chunk.
    let gemini = r#"{"candidates":[{"content":{"parts":[{"text":"secret"}]}}]}"#;
    assert_eq!(sse_event_kind(&sse("", gemini)), "?");
    // A non-string discriminant, a bare scalar, and text that is not JSON at
    // all: none of these may fail or leak, they just go unnamed.
    assert_eq!(sse_event_kind(&sse("", r#"{"type":7}"#)), "?");
    assert_eq!(sse_event_kind(&sse("", r#""content_block_delta""#)), "?");
    assert_eq!(sse_event_kind(&sse("", "[DONE]")), "?");
    assert_eq!(sse_event_kind(&sse("", "")), "?");
}

#[test]
fn structural_label_keeps_every_real_discriminant() {
    for label in [
        // Anthropic, OpenAI, Gemini, and a name no adapter models yet.
        "content_block_delta",
        "message_stop",
        "response.function_call_arguments.delta",
        "response.completed",
        "STOP",
        "MALFORMED_FUNCTION_CALL",
        "response.some_future_event.delta",
        "v2-experimental.thing",
    ] {
        assert_eq!(structural_label(label), label);
    }
}

#[test]
fn structural_label_rejects_anything_not_shaped_like_a_discriminant() {
    // Prose is what leaked content looks like: spaces and punctuation.
    for label in [
        "gemini stopped generating: the user asked about X",
        "content_filter: prompt mentioned a diagnosis",
        "here is the answer.\nsecond line",
        "{\"text\":\"secret\"}",
        // A discriminant-shaped token is not a licence to be unbounded.
        &"A".repeat(MAX_LABEL_LEN + 1),
        // Empty is not a name; `sse_event_kind` reports absence as `?` instead.
        "",
    ] {
        assert_eq!(structural_label(label), INVALID_LABEL, "kept {label:?}");
    }
    // The boundary itself is kept.
    let longest = "b".repeat(MAX_LABEL_LEN);
    assert_eq!(structural_label(&longest), longest);
}

#[test]
fn sse_event_kind_bounds_both_of_its_sources() {
    let prose = "SENTINEL MODEL OUTPUT";
    // The `event:` field.
    assert_eq!(sse_event_kind(&sse(prose, TEXT_DELTA)), INVALID_LABEL);
    // The payload discriminant, reached only when the stream names no event.
    let payload = format!(r#"{{"type":"{prose}","delta":"secret"}}"#);
    assert_eq!(sse_event_kind(&sse("", &payload)), INVALID_LABEL);
}

#[test]
fn parse_failure_never_quotes_the_value_it_rejected() {
    #[derive(Debug, serde::Deserialize)]
    struct Chunk {
        #[allow(dead_code)]
        index: u64,
    }

    // The premise: `serde_json`'s own message embeds the rejected value, which
    // on a real chunk is the model's text.
    let rejected =
        serde_json::from_str::<Chunk>(r#"{"index":"secret"}"#).expect_err("index is not a u64");
    assert!(
        rejected.to_string().contains("secret"),
        "serde_json stopped quoting rejected values; this helper may be moot"
    );
    assert_eq!(parse_failure(&rejected), "Data at line 1 column 17");

    // A truncated chunk — the common real failure. `Eof` names it on its own.
    let truncated = serde_json::from_str::<Chunk>(r#"{"index":1"#).expect_err("truncated");
    assert_eq!(parse_failure(&truncated), "Eof at line 1 column 10");

    // `from_value` has no text to point into, so no position is invented.
    let from_value = serde_json::from_value::<Chunk>(json!({"index": "secret"}))
        .expect_err("index is not a u64");
    assert_eq!(parse_failure(&from_value), "Data");
}

// ---------------------------------------------------------------------------
// Cross-adapter regression: no adapter may trace a chunk body
// ---------------------------------------------------------------------------

/// Records every `tracing` event's rendered fields, so a test can assert on what
/// did *not* reach a log.
#[derive(Clone, Default)]
struct CapturedEvents(Arc<Mutex<Vec<String>>>);

impl CapturedEvents {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().expect("captured events poisoned"))
    }
}

impl<S> Layer<S> for CapturedEvents
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: LayerContext<'_, S>) {
        struct Renderer(String);
        impl Visit for Renderer {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                let _ = write!(self.0, " {}={value:?}", field.name());
            }
        }

        let mut renderer = Renderer(event.metadata().target().to_string());
        event.record(&mut renderer);
        self.0
            .lock()
            .expect("captured events poisoned")
            .push(renderer.0);
    }
}

/// Sentinel standing in for model output. Every field an adapter could echo —
/// answer text, thinking text, tool-call arguments, thought signatures — carries
/// it, so any log line that reproduces a chunk body contains it.
///
/// Deliberately prose-shaped: real content has spaces, so a provider that puts
/// content where a discriminant belongs is caught by [`structural_label`]'s
/// bound. The transcripts below put it in those positions too.
const SENTINEL: &str = "SENTINEL MODEL OUTPUT";

fn sse_body(events: &[(&str, serde_json::Value)]) -> String {
    let mut body = String::new();
    for (name, data) in events {
        if !name.is_empty() {
            let _ = writeln!(body, "event: {name}");
        }
        let _ = write!(body, "data: {data}\n\n");
    }
    body
}

fn byte_stream(body: String) -> forall_client::ByteStream {
    Box::pin(
        ReaderStream::new(std::io::Cursor::new(body))
            .map_err(|err| TransportError::Network(err.to_string())),
    )
}

/// Drains an adapter over `body`, returning every line it traced.
///
/// The capture layer is the *thread-local* default, and a `#[tokio::test]`
/// current-thread runtime polls the spawned adapter on this same thread, so its
/// events are captured too.
async fn traced_lines<F, Fut>(body: String, run: F) -> Vec<String>
where
    F: FnOnce(forall_client::ByteStream, mpsc::Sender<Result<ResponseEvent, ApiError>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let captured = CapturedEvents::default();
    let _guard = tracing_subscriber::registry()
        .with(captured.clone())
        .set_default();

    let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
    tokio::spawn(run(byte_stream(body), tx));
    while rx.recv().await.is_some() {}

    let lines = captured.take();
    assert!(!lines.is_empty(), "the adapter traced nothing at all");
    lines
}

fn assert_no_sentinel(adapter: &str, lines: &[String]) {
    for line in lines {
        assert!(
            !line.contains(SENTINEL),
            "{adapter} traced a chunk body: {line}"
        );
    }
    // Each transcript includes a malformed chunk, so a green run that never
    // reached the parse-failure path would not have tested it.
    assert!(
        lines.iter().any(|line| line.contains("Failed to parse")),
        "{adapter} no longer reports a malformed chunk: {lines:?}"
    );
    // Likewise each transcript puts the sentinel where a discriminant belongs,
    // so the bound must have rejected something. Without this, an adapter that
    // stopped labelling events at all would pass.
    assert!(
        lines.iter().any(|line| line.contains(INVALID_LABEL)),
        "{adapter} no longer bounds a wire-supplied label: {lines:?}"
    );
}

#[tokio::test]
async fn messages_adapter_never_traces_a_chunk_body() {
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-5",
                    "content": [],
                    "usage": {"input_tokens": 1, "output_tokens": 1},
                },
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""},
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": SENTINEL},
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "signature_delta", "signature": SENTINEL},
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "tool_use", "id": "call_1", "name": "shell", "input": {}},
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": SENTINEL},
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
        // Truncated JSON: exercises the parse-failure path, which used to echo
        // the whole data line at debug level.
        ("content_block_delta", json!(SENTINEL)),
        // A provider naming the event with content rather than a discriminant.
        // Dispatch reads the body's `type`, so this parses and streams normally.
        (
            SENTINEL,
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);

    let lines = traced_lines(body, |stream, tx| {
        crate::sse::messages::process_messages_sse(
            stream,
            tx,
            Duration::from_millis(1000),
            /*telemetry*/ None,
            /*freeform_tool_names*/ Vec::new(),
        )
    })
    .await;

    assert_no_sentinel("the messages adapter", &lines);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("kind=content_block_delta")),
        "the event kind stopped being traced: {lines:?}"
    );
}

#[tokio::test]
async fn gemini_adapter_never_traces_a_chunk_body() {
    // Gemini frames are data-only, hence the empty event names.
    let body = sse_body(&[
        (
            "",
            json!({
                "responseId": "resp_1",
                "candidates": [{"content": {"parts": [
                    {"text": SENTINEL, "thought": true, "thoughtSignature": SENTINEL},
                    {"text": SENTINEL},
                    {"functionCall": {"name": "shell", "args": {"command": SENTINEL}}},
                ]}}],
            }),
        ),
        ("", json!(SENTINEL)),
        // `finishReason` is a wire enum this adapter does not constrain, so a
        // provider can put content there. It reaches both `chunk_shape` and
        // `finish_outcome`'s unknown-reason log.
        (
            "",
            json!({
                "candidates": [{"finishReason": SENTINEL}],
                "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1},
            }),
        ),
    ]);

    let lines = traced_lines(body, |stream, tx| {
        crate::sse::gemini::process_gemini_sse(
            stream,
            tx,
            Duration::from_millis(1000),
            /*telemetry*/ None,
            /*freeform_tool_names*/ Vec::new(),
        )
    })
    .await;

    assert_no_sentinel("the gemini adapter", &lines);
    assert!(
        lines.iter().any(|line| line.contains("parts=[thought,")),
        "the chunk shape stopped being traced: {lines:?}"
    );
}

#[tokio::test]
async fn responses_adapter_never_traces_a_chunk_body() {
    let body = sse_body(&[
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": SENTINEL}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"type": "response.function_call_arguments.delta", "delta": SENTINEL}),
        ),
        // A provider naming the event with content rather than a discriminant.
        // Dispatch reads the body's `type`, so this parses and streams normally.
        (
            SENTINEL,
            json!({"type": "response.reasoning_summary_text.delta", "delta": SENTINEL}),
        ),
        // An event kind this adapter does not model, then a malformed one.
        (
            "response.custom_tool_call_input.delta",
            json!({"type": "response.custom_tool_call_input.delta", "delta": SENTINEL}),
        ),
        ("response.output_text.delta", json!(SENTINEL)),
        (
            "response.completed",
            json!({
                "type": "response.completed",
                "response": {"id": "resp_1", "usage": {
                    "input_tokens": 1,
                    "input_tokens_details": {"cached_tokens": 0},
                    "output_tokens": 1,
                    "output_tokens_details": {"reasoning_tokens": 0},
                    "total_tokens": 2,
                }},
            }),
        ),
    ]);

    let lines = traced_lines(body, |stream, tx| {
        crate::sse::responses::process_sse(stream, tx, Duration::from_millis(1000), None)
    })
    .await;

    assert_no_sentinel("the responses adapter", &lines);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("kind=response.output_text.delta")),
        "the event kind stopped being traced: {lines:?}"
    );
}
