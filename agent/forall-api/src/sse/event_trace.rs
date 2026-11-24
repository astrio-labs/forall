//! Structural tracing for the SSE-framed adapters.
//!
//! Every adapter traces each event its framing loop reads, so a provider stream
//! that is malformed, truncated or out of order can be diagnosed after the
//! fact. What must not be traced is the `data:` line itself: generated text,
//! thinking blocks and tool-call inputs all arrive in there, and
//! `forall_state::log_db::default_filter` attaches TRACE-from-every-target to
//! the log-db layer as a *per-layer* filter, so a traced body is written to the
//! on-disk log database whatever `RUST_LOG` says.
//!
//! Naming the event and its byte length keeps what these traces are for. It is
//! also the sharper signal for what framing code can actually get wrong: which
//! events arrived, in what order, and where one was cut short.
//! [`crate::sse::bedrock_messages`] traces its binary frames the same way.

use eventsource_stream::Event;
use serde::Deserialize;

/// The event name `eventsource_stream` substitutes when a stream omits the
/// `event:` field, per the spec's default event type. No provider this crate
/// speaks to names a real event that, so it is read as "unnamed".
const DEFAULT_EVENT_NAME: &str = "message";

/// Longest label kept verbatim. The longest discriminant any provider here sends
/// is `response.function_call_arguments.delta` at 38 bytes; the rest is headroom
/// for event types no adapter models yet.
const MAX_LABEL_LEN: usize = 64;

/// Stands in for a label the wire supplied that is not shaped like a
/// discriminant. Distinct from `?`, which means the wire named nothing.
const INVALID_LABEL: &str = "<invalid>";

/// Bounds a provider-supplied string before it is traced as a structural label.
///
/// Event names, `type` discriminants and `finishReason`/`blockReason` values are
/// all provider-controlled, so "this field is structural" is an assumption about
/// the provider, not something the framing loop can know: a custom endpoint or
/// proxy can put anything there. A label is kept only if it is *shaped* like a
/// discriminant — 1..=[`MAX_LABEL_LEN`] bytes of ASCII alphanumerics, `_`, `-`
/// and `.` — which every real one is (`content_block_delta`,
/// `response.output_text.delta`, `STOP`, `MALFORMED_FUNCTION_CALL`).
///
/// Bounding the shape rather than enumerating known names is deliberate: an
/// allowlist would drop exactly the labels worth tracing, the event types no
/// adapter models yet, which is most of why these traces exist. What the bound
/// rules out is prose — a token this short with no spaces or punctuation cannot
/// carry generated text.
pub(crate) fn structural_label(label: &str) -> &str {
    let shaped_like_a_discriminant = !label.is_empty()
        && label.len() <= MAX_LABEL_LEN
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if shaped_like_a_discriminant {
        label
    } else {
        INVALID_LABEL
    }
}

/// Names one SSE event for tracing, without exposing its payload.
///
/// The `event:` field is preferred: Anthropic and OpenAI name every chunk there,
/// so nothing has to be parsed. When it is absent — a proxy that sends bare
/// `data:` lines — the payload's own `type` discriminant carries the same string
/// and is used instead. `?` when neither names the event; tracing must not care
/// whether the payload is well-formed, the adapter decides that.
///
/// Both sources are provider-controlled, so both go through
/// [`structural_label`].
pub(crate) fn sse_event_kind(sse: &Event) -> &str {
    if !sse.event.is_empty() && sse.event != DEFAULT_EVENT_NAME {
        return structural_label(&sse.event);
    }
    payload_kind(&sse.data).map_or("?", structural_label)
}

/// Describes a chunk that failed to parse, without quoting the chunk.
///
/// A `serde_json::Error` cannot be logged as-is: its `Display` embeds the value
/// it rejected — `invalid type: string "<the model's text>", expected ...` — so
/// a chunk whose content-bearing field has the wrong type, or a `data:` line
/// that is a bare JSON string, puts generated content in the message.
///
/// The category and position carry the diagnosis instead, and carry it more
/// legibly: `Eof` names a truncated chunk, `Syntax` a malformed one, `Data` a
/// shape the adapter's own DTOs reject, and the position says where.
pub(crate) fn parse_failure(error: &serde_json::Error) -> String {
    let category = format!("{:?}", error.classify());
    // `from_value` has no text to point into, and reports 0:0.
    if error.line() == 0 {
        return category;
    }
    format!(
        "{category} at line {} column {}",
        error.line(),
        error.column()
    )
}

/// Reads just the `type` discriminant out of a raw chunk.
///
/// The field borrows from the payload, so this scans the JSON rather than
/// materializing the chunk. The cost is that an escaped `type` value reads as
/// absent — no provider emits one, and it would only cost the trace its name.
fn payload_kind(data: &str) -> Option<&str> {
    #[derive(Deserialize)]
    struct Discriminant<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
    }

    serde_json::from_str::<Discriminant>(data)
        .ok()
        .map(|discriminant| discriminant.kind)
}

#[cfg(test)]
#[path = "event_trace_tests.rs"]
mod tests;
