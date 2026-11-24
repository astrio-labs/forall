//! Drives the Anthropic Messages translator from Bedrock's binary
//! `application/vnd.amazon.eventstream` transport.
//!
//! `bedrock-runtime`'s `InvokeModelWithResponseStream` carries exactly the
//! chunks the Anthropic SSE endpoint sends (`message_start`,
//! `content_block_*`, `message_delta`, `message_stop`) — only the framing
//! differs. So this module owns *only* framing concerns:
//!
//!   * reassembling frames with [`EventStreamDecoder`],
//!   * unwrapping the `{"bytes": "<base64>"}` payload envelope Bedrock wraps
//!     each chunk in, and
//!   * mapping event-stream exception frames onto [`ApiError`].
//!
//! Translation is delegated to [`MessagesChunkDriver`], the same state machine
//! the direct-Anthropic SSE path uses. There is no second translator.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use forall_client::ByteStream;
use forall_client::StreamResponse;
use forall_client::TransportError;
use futures::StreamExt;
use http::HeaderMap;
use http::StatusCode;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::event_stream::EventStreamDecoder;
use crate::event_stream::EventStreamFrame;
use crate::sse::event_trace::parse_failure;
use crate::sse::event_trace::structural_label;
use crate::sse::messages::ChunkFlow;
use crate::sse::messages::MessagesChunkDriver;
use crate::telemetry::SseTelemetry;

/// Bedrock returns the request id here; `bedrock-runtime` does not send
/// Anthropic's `request-id`.
const AMZN_REQUEST_ID_HEADER: &str = "x-amzn-requestid";
/// Header naming the modeled error on an HTTP-level failure, e.g.
/// `ThrottlingException:http://internal.amazon.com/coral/...`.
const AMZN_ERROR_TYPE_HEADER: &str = "x-amzn-errortype";
/// Property carrying the base64-encoded chunk inside an event frame payload.
const CHUNK_BYTES_PROPERTY: &str = "bytes";

pub fn spawn_bedrock_messages_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    freeform_tool_names: Vec<String>,
) -> ResponseStream {
    // `SseTelemetry::on_sse_poll` is typed against `eventsource_stream`'s event
    // and error types, which cannot describe a poll of a binary event stream.
    // Rather than fabricate an SSE event to satisfy the callback, per-frame poll
    // telemetry is deliberately not emitted on this transport; request-level
    // telemetry still applies.
    let _ = telemetry;
    // `bedrock-runtime` publishes no rate-limit headers (throttling is reported
    // as a `throttlingException`), so no `RateLimits` event is synthesized.
    let upstream_request_id = stream_response
        .headers
        .get(AMZN_REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_bedrock_event_stream(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            freeform_tool_names,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

pub(crate) async fn process_bedrock_event_stream(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    freeform_tool_names: Vec<String>,
) {
    let mut stream = stream;
    let mut decoder = EventStreamDecoder::new();
    let mut driver = MessagesChunkDriver::new(tx_event, freeform_tool_names);

    loop {
        // Drain everything already buffered before waiting on the transport:
        // one byte chunk routinely carries several frames.
        loop {
            match decoder.next_frame() {
                Ok(Some(frame)) => {
                    if handle_frame(frame, &mut driver).await == ChunkFlow::Stop {
                        return;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    // Framing integrity is gone: the offset of the next frame is
                    // unknown, so the stream cannot be resynchronized.
                    driver.fail(ApiError::Stream(error.to_string())).await;
                    return;
                }
            }
        }

        match timeout(idle_timeout, stream.next()).await {
            Ok(Some(Ok(bytes))) => decoder.push(&bytes),
            Ok(Some(Err(e))) => {
                debug!("bedrock event stream error: {e:#}");
                driver.fail(ApiError::Stream(e.to_string())).await;
                return;
            }
            Ok(None) => {
                if decoder.has_buffered_bytes() {
                    driver
                        .fail(ApiError::Stream(
                            "bedrock event stream closed mid-frame".into(),
                        ))
                        .await;
                } else {
                    driver.close_before_completed().await;
                }
                return;
            }
            Err(_) => {
                driver
                    .fail(ApiError::Stream(
                        "idle timeout waiting for bedrock event stream".into(),
                    ))
                    .await;
                return;
            }
        }
    }
}

async fn handle_frame(frame: EventStreamFrame, driver: &mut MessagesChunkDriver) -> ChunkFlow {
    if !frame.is_event() {
        driver.fail(api_error_from_exception_frame(&frame)).await;
        return ChunkFlow::Stop;
    }

    let Some(chunk) = chunk_from_event_payload(&frame.payload) else {
        return ChunkFlow::Continue;
    };
    // Structure only, never the chunk body: generated text, thinking blocks and
    // tool inputs all live in here, and the log-db layer captures TRACE from
    // every target by default (forall_state::log_db::default_filter). The event
    // type plus frame size is also the more useful signal for the thing this
    // module can actually get wrong — frame order and boundaries.
    trace!(
        "bedrock event stream chunk: type={} payload_bytes={}",
        chunk_type(&chunk),
        frame.payload.len()
    );
    driver.handle_chunk_value(chunk).await
}

/// The chunk's `type` discriminant, for structural tracing. `?` when absent —
/// tracing must not care whether the chunk is well-formed; the driver decides.
///
/// The value is provider-controlled, so it is bounded by [`structural_label`]
/// rather than trusted to be a discriminant.
fn chunk_type(chunk: &Value) -> &str {
    chunk
        .get("type")
        .and_then(Value::as_str)
        .map_or("?", structural_label)
}

/// Unwraps one Anthropic chunk from an event frame payload.
///
/// Bedrock wraps each chunk as `{"bytes": "<base64>", "p": "<padding>"}`. A
/// payload that is already a bare chunk object is accepted as-is so a future
/// unwrapped encoding does not silently drop content. Anything else is skipped
/// (not fatal): an unreadable frame is not evidence the message has ended.
fn chunk_from_event_payload(payload: &[u8]) -> Option<Value> {
    let value: Value = match serde_json::from_slice(payload) {
        Ok(value) => value,
        Err(e) => {
            debug!("bedrock event frame payload is not JSON: {e}");
            return None;
        }
    };
    let Value::Object(map) = &value else {
        debug!("bedrock event frame payload is not a JSON object");
        return None;
    };
    let Some(encoded) = map.get(CHUNK_BYTES_PROPERTY) else {
        // Already an unwrapped chunk.
        return Some(value);
    };
    let Some(encoded) = encoded.as_str() else {
        debug!("bedrock event frame `bytes` is not a string");
        return None;
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded) {
        Ok(decoded) => decoded,
        Err(e) => {
            debug!("bedrock event frame `bytes` is not valid base64: {e}");
            return None;
        }
    };
    match serde_json::from_slice(&decoded) {
        Ok(chunk) => Some(chunk),
        Err(e) => {
            // The decoded bytes are one Anthropic chunk, so the value this error
            // would otherwise quote is generated content; see
            // `sse::event_trace::parse_failure`.
            debug!(
                "bedrock event frame chunk is not JSON: {} (chunk_bytes={})",
                parse_failure(&e),
                decoded.len()
            );
            None
        }
    }
}

/// Maps a non-event frame (`:message-type: exception` or `error`) onto an
/// [`ApiError`].
fn api_error_from_exception_frame(frame: &EventStreamFrame) -> ApiError {
    let kind = frame.failure_kind().unwrap_or("unknown").to_string();
    let payload: Option<Value> = serde_json::from_slice(&frame.payload).ok();
    let message = frame
        .error_message
        .clone()
        .or_else(|| message_from_payload(payload.as_ref()))
        .unwrap_or_else(|| format!("bedrock stream exception: {kind}"));
    api_error_from_bedrock_error(&kind, message, retry_after_from_payload(payload.as_ref()))
}

fn message_from_payload(payload: Option<&Value>) -> Option<String> {
    let payload = payload?.as_object()?;
    // AWS is inconsistent about the casing of this member across services.
    payload
        .get("message")
        .or_else(|| payload.get("Message"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Reads a retry hint from an exception payload when the service supplied one.
///
/// Bedrock's `throttlingException` is not documented to carry a delay, so this
/// only honors one that is actually present; nothing is invented when it is
/// absent.
fn retry_after_from_payload(payload: Option<&Value>) -> Option<Duration> {
    let payload = payload?.as_object()?;
    let seconds = payload
        .get("retryAfterSeconds")
        .or_else(|| payload.get("retry_after_seconds"))?
        .as_u64()?;
    Some(Duration::from_secs(seconds))
}

/// Classifies a Bedrock error/exception name.
///
/// Retryability is decided here, mirroring
/// [`crate::sse::messages::api_error_from_messages_error`]:
///   * throttling and transient server/model failures land on
///     [`ApiError::Retryable`], the only variant `ForallErr::is_retryable`
///     accepts for these (the transport layer will not help — `retry_429` is
///     hardcoded off);
///   * validation and authorization failures land on
///     [`ApiError::InvalidRequest`], because retrying cannot fix them; and
///   * credential failures are surfaced as the HTTP-401 transport shape core's
///     unauthorized-retry loop matches on, so a stale SigV4 credential or
///     expired session token triggers a refresh instead of failing the turn.
///
/// Names are compared case-insensitively: event-stream `:exception-type`
/// headers use lowerCamelCase member names (`throttlingException`) while HTTP
/// `x-amzn-ErrorType` values use the shape name (`ThrottlingException`).
fn api_error_from_bedrock_error(kind: &str, message: String, delay: Option<Duration>) -> ApiError {
    const RETRYABLE: [&str; 6] = [
        "throttlingException",
        "modelStreamErrorException",
        "internalServerException",
        "serviceUnavailableException",
        "modelTimeoutException",
        "modelNotReadyException",
    ];
    const INVALID_REQUEST: [&str; 5] = [
        "validationException",
        "accessDeniedException",
        "resourceNotFoundException",
        "serviceQuotaExceededException",
        "modelErrorException",
    ];
    // SigV4 credential problems: a refreshed credential can fix these, so they
    // must reach core's unauthorized-retry loop rather than fail the turn.
    const UNAUTHORIZED: [&str; 6] = [
        "unrecognizedClientException",
        "invalidSignatureException",
        "expiredTokenException",
        "incompleteSignatureException",
        "missingAuthenticationTokenException",
        "unauthorizedException",
    ];

    let matches_any = |names: &[&str]| names.iter().any(|name| name.eq_ignore_ascii_case(kind));

    if matches_any(&RETRYABLE) {
        ApiError::Retryable { message, delay }
    } else if matches_any(&INVALID_REQUEST) {
        ApiError::InvalidRequest { message }
    } else if matches_any(&UNAUTHORIZED) {
        ApiError::Transport(TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: None,
            headers: None,
            body: Some(message),
        })
    } else {
        ApiError::Stream(message)
    }
}

/// Rewrites HTTP-level errors from `invoke-with-response-stream` using AWS's
/// error shape (`x-amzn-ErrorType` header and/or `{"message": ...}` body).
///
/// 401s pass through untouched so core's unauthorized-retry loop keeps matching
/// on `ApiError::Transport(TransportError::Http { status: 401, .. })`.
/// Unrecognized errors also pass through unchanged.
pub(crate) fn map_bedrock_http_error(error: ApiError) -> ApiError {
    let ApiError::Transport(TransportError::Http {
        status,
        url,
        headers,
        body,
    }) = error
    else {
        return error;
    };
    let original = |status, url, headers, body| {
        ApiError::Transport(TransportError::Http {
            status,
            url,
            headers,
            body,
        })
    };
    if status == StatusCode::UNAUTHORIZED {
        return original(status, url, headers, body);
    }

    let payload: Option<Value> = body
        .as_deref()
        .and_then(|body| serde_json::from_str(body).ok());
    let Some(kind) = error_type_from_headers(headers.as_ref())
        .or_else(|| error_type_from_payload(payload.as_ref()))
    else {
        return original(status, url, headers, body);
    };
    let message = message_from_payload(payload.as_ref())
        .or_else(|| body.clone())
        .unwrap_or_else(|| format!("bedrock error: {kind}"));
    let delay =
        retry_after_from_headers(headers.as_ref()).or(retry_after_from_payload(payload.as_ref()));

    match api_error_from_bedrock_error(&kind, message, delay) {
        // An unclassified name is not evidence about the failure: keep the raw
        // transport error so callers still see the status and body.
        ApiError::Stream(_) => original(status, url, headers, body),
        mapped => mapped,
    }
}

/// Extracts the shape name from `x-amzn-ErrorType`.
///
/// The header value can be decorated (`ThrottlingException:http://internal...`),
/// so everything from the first `:` is dropped.
fn error_type_from_headers(headers: Option<&HeaderMap>) -> Option<String> {
    let value = headers?.get(AMZN_ERROR_TYPE_HEADER)?.to_str().ok()?.trim();
    let name = value.split(':').next()?.trim();
    // Fully-qualified names appear as `com.amazonaws...#ThrottlingException`.
    let name = name.rsplit('#').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn error_type_from_payload(payload: Option<&Value>) -> Option<String> {
    let payload = payload?.as_object()?;
    let value = payload
        .get("__type")
        .or_else(|| payload.get("code"))?
        .as_str()?;
    let name = value.rsplit('#').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn retry_after_from_headers(headers: Option<&HeaderMap>) -> Option<Duration> {
    let seconds = headers?
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
#[path = "bedrock_messages_tests.rs"]
mod tests;
