//! Translates the native Gemini `streamGenerateContent` SSE stream into
//! [`ResponseEvent`]s.
//!
//! Core's turn loop is strictly single-active-item: typed deltas are only legal
//! between their own `OutputItemAdded`/`OutputItemDone`, and the stream must
//! terminate with `Completed` (or an error).
//!
//! Gemini's stream shape differs from Anthropic's in two ways that drive this
//! design:
//!
//! 1. **There are no per-part start/stop events.** Every SSE chunk is a whole
//!    `GenerateContentResponse` carrying `candidates[0].content.parts[]` with
//!    the *incremental* content. Consecutive chunks continue the same logical
//!    part when the part kind is unchanged, so this module tracks one open item
//!    and closes it the moment the part kind changes (text -> thought,
//!    thought -> function call, …). No index multiplexing is needed, and
//!    therefore no delta buffering either: parts are already sequential.
//! 2. **Function calls are not streamed incrementally.** A `functionCall` part
//!    arrives with its complete `args` object, so each one is emitted as an
//!    atomic `OutputItemAdded -> ToolCallInputDelta -> OutputItemDone` triple.
//!
//! Termination is likewise implicit: the terminal signal is a `finishReason` on
//! the candidate, not a dedicated event. A stream that ends without one yields
//! the same "closed before response.completed" error the other adapters
//! produce.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::sse::event_trace::parse_failure;
use crate::sse::event_trace::structural_label;
use base64::Engine;
use eventsource_stream::Eventsource;
use forall_client::ByteStream;
use forall_client::StreamResponse;
use forall_client::TransportError;
use forall_protocol::models::ContentItem;
use forall_protocol::models::ReasoningItemContent;
use forall_protocol::models::ResponseItem;
use forall_protocol::protocol::TokenUsage;
use futures::StreamExt;
use http::HeaderMap;
use http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

use crate::telemetry::SseTelemetry;

/// Property name that carries a freeform tool's payload on the Gemini wire.
///
/// Gemini has no grammar-constrained tool type, so the request builder
/// advertises freeform tools (e.g. `apply_patch`) as a single-string-parameter
/// function declaration. This adapter performs the inverse: a `functionCall`
/// naming one of the request's freeform tools is unwrapped back into
/// [`ResponseItem::CustomToolCall`], which is the only item shape core's
/// freeform handlers can be routed from.
///
/// Mirror of `forall_tools::GEMINI_FREEFORM_INPUT_PROPERTY`; this crate does not
/// depend on `forall-tools`, so the two definitions must stay in sync.
const FREEFORM_INPUT_PROPERTY: &str = "input";

/// Backs the synthesized response id (see [`synthesize_response_id`]).
static SYNTHETIC_RESPONSE_ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// Produces a response id for streams where Gemini omits `responseId`.
///
/// `ResponseEvent::Completed::response_id` is a plain `String` that core uses
/// for correlation and logging, so an empty value would silently collapse
/// distinct turns together. A process-local monotonic counter gives every
/// stream one stable id for its whole lifetime without adding a uuid
/// dependency; it is deliberately prefixed so a synthesized id is
/// distinguishable from a server-issued one in logs.
fn synthesize_response_id() -> String {
    let seq = SYNTHETIC_RESPONSE_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("gemini-local-{seq}")
}

pub fn spawn_gemini_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
    freeform_tool_names: Vec<String>,
) -> ResponseStream {
    // Gemini has no turn-state header; the parameter exists for signature
    // parity with `spawn_response_stream`.
    let _ = turn_state;
    // Google does not publish rate-limit headers on this endpoint (quota is
    // reported only through 429 bodies), so no `RateLimits` event is
    // synthesized here.
    let upstream_request_id = stream_response
        .headers
        .get(GOOGLE_REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_gemini_sse(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            freeform_tool_names,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

/// Google surfaces a per-request correlation id here on `generativelanguage`
/// responses.
const GOOGLE_REQUEST_ID_HEADER: &str = "x-request-id";

pub(crate) async fn process_gemini_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    freeform_tool_names: Vec<String>,
) {
    let mut stream = stream.eventsource();
    let mut state = GeminiSseState::new(freeform_tool_names);

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("SSE Error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before response.completed".into(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        let chunk: GenerateContentChunk = match serde_json::from_str(&sse.data) {
            Ok(chunk) => chunk,
            Err(e) => {
                // Not the data line, and not the error verbatim: a chunk that
                // fails to parse is usually a truncated one, so what it was
                // truncated from is generated content. See `sse::event_trace`.
                debug!(
                    "Failed to parse SSE event: {} (data_bytes={})",
                    parse_failure(&e),
                    sse.data.len()
                );
                continue;
            }
        };
        // Structure only, never the chunk body: see `sse::event_trace`. Gemini
        // names no SSE event and its chunks carry no `type` discriminant, so the
        // shape of the chunk is what stands in for one.
        trace!(
            "SSE event: {} data_bytes={}",
            chunk_shape(&chunk),
            sse.data.len()
        );

        match state.handle_chunk(chunk) {
            Ok(events) => {
                for event in events {
                    let is_completed = matches!(event, ResponseEvent::Completed { .. });
                    if tx_event.send(Ok(event)).await.is_err() {
                        return;
                    }
                    if is_completed {
                        return;
                    }
                }
            }
            Err(error) => {
                // A blocked candidate or an in-band error payload is terminal
                // for the response, but the server is not obliged to close the
                // connection afterwards. Delivering the error now (rather than
                // waiting for close) keeps the consumer from burning the full
                // stream idle timeout before it can retry.
                let _ = tx_event.send(Err(error.into_api_error())).await;
                return;
            }
        }
    }
}

/// Summarizes a chunk's structure for tracing.
///
/// Gemini has no per-event type to name: every chunk is a whole
/// `GenerateContentResponse`, and which parts it carries is the closest thing to
/// a discriminant. Only counts and wire-level enum names appear here — never
/// part text, thought text, thought signatures or function-call arguments.
fn chunk_shape(chunk: &GenerateContentChunk) -> String {
    let mut shape = format!("candidates={}", chunk.candidates.len());
    // Only the first candidate is consulted, matching `handle_chunk`.
    if let Some(candidate) = chunk.candidates.first() {
        let parts = candidate
            .content
            .as_ref()
            .map(|content| content.parts.as_slice())
            .unwrap_or_default();
        shape.push_str(" parts=[");
        for (index, part) in parts.iter().enumerate() {
            if index > 0 {
                shape.push(',');
            }
            shape.push_str(part_kind(part));
        }
        shape.push(']');
        if let Some(finish_reason) = candidate.finish_reason.as_deref() {
            shape.push_str(" finish_reason=");
            shape.push_str(structural_label(finish_reason));
        }
    }
    if chunk.usage_metadata.is_some() {
        shape.push_str(" usage");
    }
    if let Some(block_reason) = chunk
        .prompt_feedback
        .as_ref()
        .and_then(|feedback| feedback.block_reason.as_deref())
    {
        shape.push_str(" block_reason=");
        shape.push_str(structural_label(block_reason));
    }
    if chunk.error.is_some() {
        shape.push_str(" error");
    }
    shape
}

/// Which kind of part this is, in the order [`GeminiSseState::handle_part`]
/// tests the fields — so the trace names the branch the driver took. `?` for a
/// part carrying none of them, which that method logs and ignores.
fn part_kind(part: &GeminiPartWire) -> &'static str {
    if part.function_call.is_some() {
        "function_call"
    } else if part.text.is_some() {
        if part.thought == Some(true) {
            "thought"
        } else {
            "text"
        }
    } else if part.thought_signature.is_some() {
        "thought_signature"
    } else if part.inline_data.is_some() {
        "inline_data"
    } else {
        "?"
    }
}

// ---------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------

/// One `GenerateContentResponse` chunk.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenerateContentChunk {
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default)]
    usage_metadata: Option<GeminiUsageMetadata>,
    #[serde(default)]
    prompt_feedback: Option<GeminiPromptFeedback>,
    /// Gemini can deliver a transport-style error as a stream payload instead
    /// of an HTTP status when the failure happens after headers are sent.
    #[serde(default)]
    error: Option<GeminiErrorBody>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiContentWire>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiContentWire {
    #[serde(default)]
    parts: Vec<GeminiPartWire>,
}

/// A `Part` is a message with mutually exclusive optional fields rather than a
/// tagged union, so every field is optional here too.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiPartWire {
    #[serde(default)]
    text: Option<String>,
    /// `true` marks the `text` as a reasoning summary rather than answer text.
    #[serde(default)]
    thought: Option<bool>,
    #[serde(default)]
    thought_signature: Option<String>,
    #[serde(default)]
    function_call: Option<GeminiFunctionCallWire>,
    #[serde(default)]
    inline_data: Option<GeminiBlobWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallWire {
    /// Present only on API versions that issue call ids; synthesized otherwise.
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    args: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiBlobWire {
    #[serde(default)]
    mime_type: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: Option<i64>,
    #[serde(default)]
    candidates_token_count: Option<i64>,
    #[serde(default)]
    total_token_count: Option<i64>,
    #[serde(default)]
    cached_content_token_count: Option<i64>,
    #[serde(default)]
    thoughts_token_count: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiPromptFeedback {
    #[serde(default)]
    block_reason: Option<String>,
}

/// Google's canonical error body: `{"error": {"code", "message", "status"}}`.
#[derive(Debug, Deserialize, Default)]
pub(crate) struct GeminiErrorBody {
    #[serde(default)]
    code: Option<u16>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorEnvelope {
    error: GeminiErrorBody,
}

#[derive(Debug)]
pub(crate) enum GeminiEventError {
    Api(ApiError),
}

impl GeminiEventError {
    pub(crate) fn into_api_error(self) -> ApiError {
        match self {
            Self::Api(error) => error,
        }
    }
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

/// The single item currently open (its `OutputItemAdded` emitted, its
/// `OutputItemDone` not yet).
///
/// Function calls never appear here: they arrive complete and are emitted as a
/// closed triple.
#[derive(Debug)]
enum ActiveItem {
    Text {
        text: String,
    },
    Thought {
        text: String,
        signature: Option<String>,
    },
}

#[derive(Debug, Default)]
struct UsageAccumulator {
    /// Gemini `promptTokenCount` — INCLUDES the cached prefix, matching
    /// OpenAI's convention and unlike Anthropic's.
    prompt_tokens: i64,
    cached_content_tokens: i64,
    /// Visible answer tokens only; thinking is counted separately.
    candidates_tokens: i64,
    thoughts_tokens: i64,
    /// Reported by Gemini directly, so it is preferred over a synthesized sum.
    total_tokens: Option<i64>,
    seen: bool,
}

impl UsageAccumulator {
    /// Assembles a [`TokenUsage`] from Gemini's `usageMetadata`.
    ///
    /// Cache convention: `promptTokenCount` already includes
    /// `cachedContentTokenCount`, so — unlike the Messages adapter, which must
    /// SUM its three input fields because Anthropic's `input_tokens` excludes
    /// cache reads — the prompt count is used verbatim and the cached count is
    /// reported only as the cached subset. Folding it in again would
    /// double-count the cached prefix and push the auto-compaction threshold
    /// (90% of the context window) off in both directions.
    ///
    /// Output convention: `candidatesTokenCount` counts only visible answer
    /// tokens while `thoughtsTokenCount` is reported separately. The Responses
    /// convention is for `output_tokens` to be reasoning-inclusive, so the two
    /// are summed and thinking is additionally reported as
    /// `reasoning_output_tokens`.
    fn to_token_usage(&self) -> Option<TokenUsage> {
        if !self.seen {
            return None;
        }
        let output_tokens = self.candidates_tokens + self.thoughts_tokens;
        Some(TokenUsage {
            input_tokens: self.prompt_tokens,
            cached_input_tokens: self.cached_content_tokens,
            output_tokens,
            reasoning_output_tokens: self.thoughts_tokens,
            total_tokens: self
                .total_tokens
                .unwrap_or(self.prompt_tokens + output_tokens),
        })
    }

    /// `usageMetadata` is cumulative on every chunk, so fields are assigned
    /// rather than accumulated.
    fn absorb(&mut self, usage: &GeminiUsageMetadata) {
        self.seen = true;
        if let Some(value) = usage.prompt_token_count {
            self.prompt_tokens = value;
        }
        if let Some(value) = usage.cached_content_token_count {
            self.cached_content_tokens = value;
        }
        if let Some(value) = usage.candidates_token_count {
            self.candidates_tokens = value;
        }
        if let Some(value) = usage.thoughts_token_count {
            self.thoughts_tokens = value;
        }
        if let Some(value) = usage.total_token_count {
            self.total_tokens = Some(value);
        }
    }
}

#[derive(Debug)]
pub(crate) struct GeminiSseState {
    /// Server-issued `responseId` when present.
    response_id: Option<String>,
    /// Stable fallback used when the server never sends `responseId`.
    synthetic_response_id: String,
    usage: UsageAccumulator,
    active: Option<ActiveItem>,
    /// Whether any `functionCall` part was seen. A response containing tool
    /// calls always needs a follow-up turn, so this overrides the
    /// `finishReason` mapping.
    saw_function_call: bool,
    /// Counter behind synthesized tool-call ids.
    function_call_seq: u64,
    created_emitted: bool,
    freeform_tool_names: HashSet<String>,
}

impl GeminiSseState {
    pub(crate) fn new(freeform_tool_names: Vec<String>) -> Self {
        Self {
            response_id: None,
            synthetic_response_id: synthesize_response_id(),
            usage: UsageAccumulator::default(),
            active: None,
            saw_function_call: false,
            function_call_seq: 0,
            created_emitted: false,
            freeform_tool_names: freeform_tool_names.into_iter().collect(),
        }
    }

    fn response_id(&self) -> String {
        self.response_id
            .clone()
            .unwrap_or_else(|| self.synthetic_response_id.clone())
    }

    pub(crate) fn handle_chunk(
        &mut self,
        chunk: GenerateContentChunk,
    ) -> Result<Vec<ResponseEvent>, GeminiEventError> {
        if let Some(error) = chunk.error {
            return Err(GeminiEventError::Api(api_error_from_gemini_error(
                &error, /*delay*/ None,
            )));
        }

        let mut out = Vec::new();
        if !self.created_emitted {
            self.created_emitted = true;
            out.push(ResponseEvent::Created);
        }
        if let Some(response_id) = chunk.response_id {
            self.response_id = Some(response_id);
        }
        if let Some(usage) = chunk.usage_metadata.as_ref() {
            self.usage.absorb(usage);
        }
        // A blocked *prompt* comes back as `promptFeedback.blockReason` with no
        // candidates; surfacing it as an error keeps a refused request from
        // looking like an empty successful turn.
        if let Some(block_reason) = chunk
            .prompt_feedback
            .as_ref()
            .and_then(|feedback| feedback.block_reason.as_deref())
            && !block_reason.is_empty()
        {
            return Err(GeminiEventError::Api(ApiError::InvalidRequest {
                message: format!("gemini blocked the prompt: {block_reason}"),
            }));
        }

        // Only the first candidate is consulted: core has one active item and
        // the request never asks for more than one candidate.
        let Some(candidate) = chunk.candidates.into_iter().next() else {
            return Ok(out);
        };
        if let Some(content) = candidate.content {
            for part in content.parts {
                self.handle_part(part, &mut out);
            }
        }

        if let Some(finish_reason) = candidate.finish_reason.as_deref() {
            self.close_active(&mut out);
            let end_turn = match finish_outcome(finish_reason, self.saw_function_call) {
                FinishOutcome::EndTurn(end_turn) => end_turn,
                FinishOutcome::Fatal(error) => return Err(GeminiEventError::Api(error)),
            };
            out.push(ResponseEvent::Completed {
                response_id: self.response_id(),
                token_usage: self.usage.to_token_usage(),
                end_turn,
            });
        }

        Ok(out)
    }

    fn handle_part(&mut self, part: GeminiPartWire, out: &mut Vec<ResponseEvent>) {
        if let Some(function_call) = part.function_call {
            // A function-call part also carries a `thoughtSignature` on
            // thinking models. `ResponseItem::FunctionCall` has no field able
            // to hold it, so it cannot be replayed; see the module docs on
            // `build_gemini_request` for the resulting fidelity gap.
            //
            // Why not simply add a field: `ResponseItem` is the serialization
            // unit for EVERY provider — the Responses request sends it verbatim
            // as `input` — and there is no per-provider filtering at
            // serialization time. A new field would therefore appear in OpenAI
            // request bodies too. Carrying this needs a provider-scoped side
            // channel that also survives rollout persistence (Anthropic and
            // Bedrock want the same thing).
            //
            // ESCALATION CRITERION: this is a fidelity gap only while Gemini
            // degrades gracefully without the signature. If a thinking model
            // with tool use HARD-REJECTS the omission, it becomes a correctness
            // bug and the side channel above stops being optional. That
            // distinction has not been tested against a live endpoint; do it
            // before anyone ships Gemini + thinking + tools to users.
            if part.thought_signature.is_some() {
                debug!(
                    "dropping thoughtSignature on a functionCall part: ResponseItem::FunctionCall has nowhere to carry it"
                );
            }
            self.close_active(out);
            self.emit_function_call(function_call, out);
            return;
        }

        if let Some(text) = part.text {
            let is_thought = part.thought == Some(true);
            if is_thought {
                self.handle_thought_text(text, part.thought_signature, out);
            } else {
                self.handle_answer_text(text, out);
            }
            return;
        }

        // A bare signature with no text: nothing is open to attach it to often
        // enough that this is only logged.
        if let Some(signature) = part.thought_signature {
            match self.active.as_mut() {
                Some(ActiveItem::Thought {
                    signature: slot, ..
                }) => *slot = Some(signature),
                _ => debug!("ignoring thoughtSignature with no open reasoning item"),
            }
            return;
        }

        if let Some(blob) = part.inline_data {
            // Generated images have no faithful `ResponseItem` representation
            // here (`ImageGenerationCall` models the Responses hosted tool, not
            // an inline blob), and the Gemini catalog does not advertise image
            // generation, so this is only reachable if a model volunteers one.
            debug!(
                "ignoring inlineData part of type {:?}: no ResponseItem representation",
                blob.mime_type
            );
            return;
        }

        debug!("ignoring unrecognized gemini part");
    }

    fn handle_answer_text(&mut self, text: String, out: &mut Vec<ResponseEvent>) {
        if text.is_empty() {
            // Opening an item for an empty part would emit an empty assistant
            // message; skipping keeps `OutputItemAdded`/`Done` paired.
            return;
        }
        if !matches!(self.active, Some(ActiveItem::Text { .. })) {
            self.close_active(out);
            self.active = Some(ActiveItem::Text {
                text: String::new(),
            });
            out.push(ResponseEvent::OutputItemAdded(empty_message_item()));
        }
        if let Some(ActiveItem::Text { text: buffer }) = self.active.as_mut() {
            buffer.push_str(&text);
        }
        out.push(ResponseEvent::OutputTextDelta(text));
    }

    fn handle_thought_text(
        &mut self,
        text: String,
        signature: Option<String>,
        out: &mut Vec<ResponseEvent>,
    ) {
        if text.is_empty() && signature.is_none() {
            return;
        }
        if !matches!(self.active, Some(ActiveItem::Thought { .. })) {
            self.close_active(out);
            self.active = Some(ActiveItem::Thought {
                text: String::new(),
                signature: None,
            });
            out.push(ResponseEvent::OutputItemAdded(empty_reasoning_item()));
        }
        if let Some(ActiveItem::Thought {
            text: buffer,
            signature: slot,
        }) = self.active.as_mut()
        {
            buffer.push_str(&text);
            if let Some(signature) = signature {
                *slot = Some(signature);
            }
        }
        if !text.is_empty() {
            out.push(ResponseEvent::ReasoningContentDelta {
                delta: text,
                content_index: 0,
            });
        }
    }

    /// Emits the complete `OutputItemAdded -> ToolCallInputDelta ->
    /// OutputItemDone` triple for one `functionCall` part.
    fn emit_function_call(
        &mut self,
        function_call: GeminiFunctionCallWire,
        out: &mut Vec<ResponseEvent>,
    ) {
        self.saw_function_call = true;
        let GeminiFunctionCallWire { id, name, args } = function_call;
        let call_id = id.filter(|id| !id.is_empty()).unwrap_or_else(|| {
            let seq = self.function_call_seq;
            self.function_call_seq += 1;
            // Gemini matches tool results to calls by function name, so this id
            // is purely internal: the request builder resolves the name back
            // out of history by call id. It only has to be unique within the
            // response.
            format!("{}-call-{seq}", self.response_id())
        });
        // Core parses `FunctionCall::arguments` as a JSON *string*, so the args
        // object is re-stringified. A call with no arguments still gets `{}`.
        let arguments = args.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        let arguments = arguments.to_string();

        if self.freeform_tool_names.contains(&name) {
            // The tool was advertised as a one-property object tool, so unwrap
            // that property back into the raw payload text: core routes
            // freeform handlers from `CustomToolCall` only.
            //
            // If the model did not honor that shape, still emit
            // `CustomToolCall` with the raw arguments as the payload. A
            // `FunctionCall` here would be structurally undispatchable — the
            // freeform handler rejects non-custom payloads outright — so the
            // turn would die on a fatal incompatible-payload error. Handing the
            // handler the raw text instead yields at worst a recoverable parse
            // error the model can retry.
            let input =
                freeform_input_from_arguments(&arguments).unwrap_or_else(|| arguments.clone());
            out.push(ResponseEvent::OutputItemAdded(
                ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: call_id.clone(),
                    name: name.clone(),
                    input: String::new(),
                    internal_chat_message_metadata_passthrough: None,
                },
            ));
            out.push(ResponseEvent::ToolCallInputDelta {
                item_id: call_id.clone(),
                call_id: Some(call_id.clone()),
                delta: input.clone(),
            });
            out.push(ResponseEvent::OutputItemDone(
                ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id,
                    name,
                    input,
                    internal_chat_message_metadata_passthrough: None,
                },
            ));
            return;
        }

        out.push(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall {
            id: None,
            name: name.clone(),
            namespace: None,
            arguments: String::new(),
            call_id: call_id.clone(),
            internal_chat_message_metadata_passthrough: None,
        }));
        out.push(ResponseEvent::ToolCallInputDelta {
            item_id: call_id.clone(),
            call_id: Some(call_id.clone()),
            delta: arguments.clone(),
        });
        out.push(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            id: None,
            name,
            namespace: None,
            arguments,
            call_id,
            internal_chat_message_metadata_passthrough: None,
        }));
    }

    /// Emits `OutputItemDone` for the open item, if any.
    fn close_active(&mut self, out: &mut Vec<ResponseEvent>) {
        let Some(active) = self.active.take() else {
            return;
        };
        let item = match active {
            ActiveItem::Text { text } => ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ActiveItem::Thought { text, signature } => ResponseItem::Reasoning {
                id: None,
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: text.clone(),
                }]),
                // Only a signed thought is worth replaying: the envelope is
                // what lets the request builder rebuild the part with its
                // signature. Without one there is nothing to preserve, so the
                // slot stays empty and the item is dropped from history on the
                // next turn rather than replayed unsigned.
                encrypted_content: signature
                    .as_deref()
                    .map(|signature| thought_envelope(&text, signature)),
                internal_chat_message_metadata_passthrough: None,
            },
        };
        out.push(ResponseEvent::OutputItemDone(item));
    }
}

fn empty_message_item() -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: Vec::new(),
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn empty_reasoning_item() -> ResponseItem {
    ResponseItem::Reasoning {
        id: None,
        summary: Vec::new(),
        content: None,
        encrypted_content: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

/// Round-trip envelope stored in `ResponseItem::Reasoning::encrypted_content`.
///
/// A thought part is only accepted back by Gemini together with the
/// `thoughtSignature` it was issued with, so both halves travel as one opaque
/// blob; the request builder decodes it back into a thought part.
fn thought_envelope(text: &str, signature: &str) -> String {
    let envelope = serde_json::json!({
        "text": text,
        "thoughtSignature": signature,
    });
    base64::engine::general_purpose::STANDARD.encode(envelope.to_string())
}

/// Extracts a freeform tool's raw payload from the single-property object the
/// Gemini request advertised.
///
/// Returns `None` when the model did not honor that shape (missing property,
/// non-string value, extra keys), in which case the caller passes the raw
/// arguments through rather than fabricating a payload.
fn freeform_input_from_arguments(arguments: &str) -> Option<String> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(arguments) else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    match map.get(FREEFORM_INPUT_PROPERTY) {
        Some(Value::String(input)) => Some(input.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// finishReason mapping
// ---------------------------------------------------------------------------

enum FinishOutcome {
    EndTurn(Option<bool>),
    Fatal(ApiError),
}

/// Maps a candidate `finishReason` onto `Completed::end_turn`, or onto a fatal
/// error for the reasons that mean "generation was cut off" rather than
/// "generation finished".
///
/// A response containing tool calls always needs a follow-up turn, so
/// `saw_function_call` wins over the reason itself (Gemini reports plain `STOP`
/// alongside `functionCall` parts).
fn finish_outcome(finish_reason: &str, saw_function_call: bool) -> FinishOutcome {
    match finish_reason {
        "STOP" => FinishOutcome::EndTurn(Some(!saw_function_call)),
        // Truncation is not a turn end. Reporting Completed here would make core
        // persist a half-written answer as the finished turn. The Responses
        // adapter treats `response.incomplete` the same way (ApiError::Stream),
        // so truncation surfaces to the user and the stream-retry layer decides.
        "MAX_TOKENS" => FinishOutcome::Fatal(ApiError::Stream(format!(
            "gemini response truncated at max output tokens (finishReason: {finish_reason})"
        ))),
        // Content-policy and safety stops discard part of the answer. Reporting
        // them as a normal end would silently hand core a truncated turn, so
        // they surface as errors carrying the reason.
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY"
        | "LANGUAGE" => FinishOutcome::Fatal(ApiError::InvalidRequest {
            message: format!("gemini stopped generating: {finish_reason}"),
        }),
        // The model emitted a tool call that does not match any declaration.
        // Non-retryable: replaying the identical request reproduces it.
        "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" => {
            FinishOutcome::Fatal(ApiError::InvalidRequest {
                message: format!("gemini stopped generating: {finish_reason}"),
            })
        }
        // Unspecified server-side failure. Mapped to `Stream` (not
        // `InvalidRequest`) so the stream-retry layer gets one chance at it.
        "OTHER" => FinishOutcome::Fatal(ApiError::Stream(
            "gemini stopped generating: OTHER".to_string(),
        )),
        other => {
            // Unmatched, so unlike the arms above this is not a known enum name.
            debug!("unknown gemini finishReason: {}", structural_label(other));
            FinishOutcome::EndTurn(saw_function_call.then_some(false))
        }
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Maps a Google error payload to an [`ApiError`].
///
/// Retryability is decided here: `RESOURCE_EXHAUSTED` (429) and `UNAVAILABLE`
/// (503) must land on `ApiError::Retryable` — mapping them to `ServerOverloaded`
/// would make them fatal (`ForallErr::is_retryable` excludes it), and the
/// transport layer will not help because `retry_429` is hardcoded off.
/// `UNAUTHENTICATED`/`PERMISSION_DENIED` are surfaced as the HTTP-401 transport
/// shape the core retry loop matches on for token refresh; that loop is bounded
/// by the recovery chain, so a static API key fails fast instead of looping.
pub(crate) fn api_error_from_gemini_error(
    error: &GeminiErrorBody,
    delay: Option<Duration>,
) -> ApiError {
    let message = error.message.clone();
    match (error.status.as_str(), error.code) {
        ("RESOURCE_EXHAUSTED", _) | (_, Some(429)) => ApiError::Retryable { message, delay },
        ("UNAVAILABLE", _) | (_, Some(503)) => ApiError::Retryable { message, delay },
        ("INVALID_ARGUMENT" | "FAILED_PRECONDITION" | "NOT_FOUND", _) | (_, Some(400 | 404)) => {
            ApiError::InvalidRequest { message }
        }
        // Only a genuinely unauthenticated response should reach core's
        // unauthorized-retry loop. Google also returns PERMISSION_DENIED/403 for
        // billing, API-not-enabled, project configuration and regional
        // restrictions — a key refresh cannot fix any of those, and rewriting
        // them to 401 replaces Google's actionable message with a generic
        // "unauthenticated" failure.
        ("UNAUTHENTICATED", _) | (_, Some(401)) => ApiError::Transport(TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: None,
            headers: None,
            body: Some(message),
        }),
        ("PERMISSION_DENIED", _) | (_, Some(403)) => ApiError::InvalidRequest { message },
        _ => ApiError::Stream(message),
    }
}

/// Rewrites HTTP-level errors from the Gemini endpoint using Google's error
/// body (`{"error": {"code", "message", "status"}}`).
///
/// 401s pass through untouched so the core unauthorized-retry loop keeps
/// matching on `ApiError::Transport(TransportError::Http { status: 401, .. })`.
/// Unrecognized bodies also pass through unchanged.
pub(crate) fn map_gemini_http_error(error: ApiError) -> ApiError {
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
    let Some(parsed) = body.as_deref().and_then(parse_gemini_error_body) else {
        return original(status, url, headers, body);
    };
    match api_error_from_gemini_error(&parsed, retry_after_from_headers(headers.as_ref())) {
        // Nothing recognizable in the body: keep the original transport error
        // so the status code and headers survive for diagnostics.
        ApiError::Stream(_) => original(status, url, headers, body),
        mapped => mapped,
    }
}

/// Parses Google's error body, accepting both the bare object and the
/// single-element array form `streamGenerateContent` uses when it fails after
/// the response has started.
///
/// The shape is discriminated through `Value` rather than by trying the object
/// form first: serde deserializes a struct from a JSON *sequence* as well as a
/// map, so `[{"error": …}]` would otherwise parse as an envelope whose `error`
/// field is the whole first element — yielding a silently defaulted (empty)
/// error instead of falling through to the array branch.
fn parse_gemini_error_body(body: &str) -> Option<GeminiErrorBody> {
    let value: Value = serde_json::from_str(body).ok()?;
    let envelope = match value {
        Value::Array(items) => items.into_iter().next()?,
        value => value,
    };
    serde_json::from_value::<GeminiErrorEnvelope>(envelope)
        .ok()
        .map(|envelope| envelope.error)
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
#[path = "gemini_tests.rs"]
mod tests;
