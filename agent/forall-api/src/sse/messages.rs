//! Translates the Anthropic Messages SSE stream into [`ResponseEvent`]s.
//!
//! Core's turn loop is strictly single-active-item: typed deltas are only
//! legal between their own `OutputItemAdded`/`OutputItemDone`, and the stream
//! must terminate with `Completed` (or an error). Anthropic multiplexes
//! content blocks by integer `index` inside one message, so this module owns a
//! per-index state machine that re-serializes blocks into sequential
//! `OutputItemAdded -> deltas -> OutputItemDone` triples.
//!
//! Fidelity trade-off: blocks are activated in the order their
//! `content_block_start` arrives, and deltas for a non-active block are
//! buffered until every earlier block has finished. If a server ever
//! interleaves deltas across open blocks, the later blocks stop streaming
//! live and are flushed as complete triples instead — content is preserved,
//! only incremental rendering latency is lost.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::sse::event_trace::parse_failure;
use crate::sse::event_trace::sse_event_kind;
use base64::Engine;
use eventsource_stream::Eventsource;
use forall_client::ByteStream;
use forall_client::StreamResponse;
use forall_client::TransportError;
use forall_protocol::models::ContentItem;
use forall_protocol::models::ReasoningItemContent;
use forall_protocol::models::ResponseItem;
use forall_protocol::protocol::RateLimitSnapshot;
use forall_protocol::protocol::RateLimitWindow;
use forall_protocol::protocol::TokenUsage;
use futures::StreamExt;
use http::HeaderMap;
use http::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

use crate::telemetry::SseTelemetry;

/// Anthropic sends the canonical request id in `request-id`.
const ANTHROPIC_REQUEST_ID_HEADER: &str = "request-id";
/// Some proxies surface it as `x-request-id` instead.
const FALLBACK_REQUEST_ID_HEADER: &str = "x-request-id";

/// Property name that carries a freeform tool's payload on the Messages wire.
///
/// The Messages API has no grammar-constrained tool type, so the request
/// builder advertises freeform tools (e.g. `apply_patch`) as a single-string
/// parameter object tool. This adapter performs the inverse: a `tool_use` block
/// naming one of the request's freeform tools is unwrapped back into
/// [`ResponseItem::CustomToolCall`], which is the only item shape core's
/// freeform handlers can be routed from.
///
/// Mirror of `forall_tools::MESSAGES_FREEFORM_INPUT_PROPERTY`; this crate does
/// not depend on `forall-tools`, so the two definitions must stay in sync.
const FREEFORM_INPUT_PROPERTY: &str = "input";

pub fn spawn_messages_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    turn_state: Option<Arc<OnceLock<String>>>,
    freeform_tool_names: Vec<String>,
) -> ResponseStream {
    // The Messages API has no turn-state header; the parameter exists for
    // signature parity with `spawn_response_stream`.
    let _ = turn_state;
    let rate_limit_snapshots = parse_anthropic_rate_limits(&stream_response.headers);
    let upstream_request_id = stream_response
        .headers
        .get(ANTHROPIC_REQUEST_ID_HEADER)
        .or_else(|| stream_response.headers.get(FALLBACK_REQUEST_ID_HEADER))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        for snapshot in rate_limit_snapshots {
            let _ = tx_event.send(Ok(ResponseEvent::RateLimits(snapshot))).await;
        }
        process_messages_sse(
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

pub(crate) async fn process_messages_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    freeform_tool_names: Vec<String>,
) {
    let mut stream = stream.eventsource();
    let mut driver = MessagesChunkDriver::new(tx_event, freeform_tool_names);

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
                driver.fail(ApiError::Stream(e.to_string())).await;
                return;
            }
            Ok(None) => {
                driver.close_before_completed().await;
                return;
            }
            Err(_) => {
                driver
                    .fail(ApiError::Stream("idle timeout waiting for SSE".into()))
                    .await;
                return;
            }
        };

        // Structure only, never the data line: see `sse::event_trace`.
        trace!(
            "SSE event: kind={} data_bytes={}",
            sse_event_kind(&sse),
            sse.data.len()
        );

        if driver.handle_chunk_str(&sse.data).await == ChunkFlow::Stop {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Framing-agnostic driver
// ---------------------------------------------------------------------------

/// Whether the caller should keep feeding chunks to a [`MessagesChunkDriver`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkFlow {
    /// Keep reading the transport.
    Continue,
    /// The turn finished, a terminal error was delivered, or the consumer hung
    /// up. Stop reading the transport; nothing further will be delivered.
    Stop,
}

/// Drives [`MessagesSseState`] from a source of already-parsed Anthropic
/// streaming chunks, independent of how those chunks were framed on the wire.
///
/// Two transports carry byte-identical chunk JSON:
///   * Anthropic's `text/event-stream` (see [`process_messages_sse`]), and
///   * Bedrock's binary `application/vnd.amazon.eventstream` (see
///     [`crate::sse::bedrock_messages`]), whose frame payloads base64-decode to
///     exactly one of these chunks.
///
/// Only the framing differs, so the translation — the per-index state machine,
/// the completion/termination semantics, and the premature-close error — lives
/// here once rather than being duplicated per transport.
pub(crate) struct MessagesChunkDriver {
    state: MessagesSseState,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
}

impl MessagesChunkDriver {
    pub(crate) fn new(
        tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
        freeform_tool_names: Vec<String>,
    ) -> Self {
        Self {
            state: MessagesSseState::new(freeform_tool_names),
            tx_event,
        }
    }

    /// Feeds one chunk from its raw JSON text.
    ///
    /// Text that is not a well-formed streaming chunk is skipped rather than
    /// failing the turn: the wire may gain event types this adapter does not
    /// model, and an unreadable frame is not evidence the message is over.
    pub(crate) async fn handle_chunk_str(&mut self, data: &str) -> ChunkFlow {
        let event: MessagesStreamEvent = match serde_json::from_str(data) {
            Ok(event) => event,
            Err(e) => {
                // Not the data line either, and not the error verbatim: a chunk
                // that fails to parse is usually a truncated one, so what it was
                // truncated from is generated content. See `sse::event_trace`.
                debug!(
                    "Failed to parse SSE event: {} (data_bytes={})",
                    parse_failure(&e),
                    data.len()
                );
                return ChunkFlow::Continue;
            }
        };
        self.dispatch(event).await
    }

    /// Feeds one chunk that has already been parsed into a [`Value`], as the
    /// event-stream decoder produces.
    pub(crate) async fn handle_chunk_value(&mut self, chunk: Value) -> ChunkFlow {
        let event: MessagesStreamEvent = match serde_json::from_value(chunk) {
            Ok(event) => event,
            Err(e) => {
                // `from_value` quotes the value it rejected just as `from_str`
                // does, so this reaches the Bedrock transport too.
                debug!("Failed to parse messages chunk: {}", parse_failure(&e));
                return ChunkFlow::Continue;
            }
        };
        self.dispatch(event).await
    }

    async fn dispatch(&mut self, event: MessagesStreamEvent) -> ChunkFlow {
        match self.state.handle_event(event) {
            Ok(events) => {
                for event in events {
                    let is_completed = matches!(event, ResponseEvent::Completed { .. });
                    if self.tx_event.send(Ok(event)).await.is_err() {
                        return ChunkFlow::Stop;
                    }
                    if is_completed {
                        return ChunkFlow::Stop;
                    }
                }
                ChunkFlow::Continue
            }
            Err(error) => {
                // An Anthropic `error` event is terminal for the message, but
                // the server is not obliged to close the connection after it.
                // Deferring delivery to the close (as the Responses path can,
                // because that transport does close) would make the consumer
                // wait out the full stream idle timeout before retrying, so
                // the error is delivered now and processing stops.
                self.fail(error.into_api_error()).await;
                ChunkFlow::Stop
            }
        }
    }

    /// Delivers a terminal error for this stream.
    pub(crate) async fn fail(&mut self, error: ApiError) {
        let _ = self.tx_event.send(Err(error)).await;
    }

    /// Reports a transport that ended without a `message_stop`.
    pub(crate) async fn close_before_completed(&mut self) {
        self.fail(ApiError::Stream(
            "stream closed before response.completed".into(),
        ))
        .await;
    }
}

// ---------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum MessagesStreamEvent {
    MessageStart {
        message: MessageStartBody,
    },
    ContentBlockStart {
        index: u64,
        content_block: ContentBlockStart,
    },
    ContentBlockDelta {
        index: u64,
        delta: ContentBlockDelta,
    },
    ContentBlockStop {
        index: u64,
    },
    MessageDelta {
        #[serde(default)]
        delta: Option<MessageDeltaBody>,
        #[serde(default)]
        usage: Option<MessageDeltaUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: MessagesErrorBody,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageStartBody {
    #[serde(default)]
    id: String,
    #[serde(default)]
    usage: Option<MessageStartUsage>,
}

#[derive(Debug, Deserialize)]
struct MessageStartUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cache_creation_input_tokens: Option<i64>,
    #[serde(default)]
    cache_read_input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking {
        #[serde(default)]
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ContentBlockDelta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaBody {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessageDeltaUsage {
    #[serde(default)]
    output_tokens: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MessagesErrorBody {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: String,
}

/// HTTP error body shape: `{"type": "error", "error": {...}}`.
#[derive(Debug, Deserialize)]
struct MessagesErrorEnvelope {
    error: MessagesErrorBody,
}

#[derive(Debug)]
pub(crate) enum MessagesEventError {
    Api(ApiError),
}

impl MessagesEventError {
    pub(crate) fn into_api_error(self) -> ApiError {
        match self {
            Self::Api(error) => error,
        }
    }
}

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum PartialBlockKind {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        /// Accumulated `input_json_delta.partial_json` fragments.
        json_fragments: String,
        /// The `input` object from `content_block_start` (usually `{}`); used
        /// when no fragments arrive.
        start_input: Value,
        /// Whether the request advertised this tool as a freeform tool, in
        /// which case the block is re-emitted as a `CustomToolCall`.
        freeform: bool,
    },
    /// Block type this adapter does not understand; consumed silently.
    Unknown,
}

/// Delta buffered for a block that is not currently active.
#[derive(Debug)]
enum BufferedDelta {
    Text(String),
    Thinking(String),
    ToolJson(String),
}

#[derive(Debug)]
struct PartialBlock {
    kind: PartialBlockKind,
    buffered: Vec<BufferedDelta>,
    stopped: bool,
}

#[derive(Debug, Default)]
struct UsageAccumulator {
    /// Anthropic `input_tokens` — EXCLUDES cache reads and cache writes,
    /// which is the opposite of OpenAI's convention.
    input_tokens: i64,
    cache_creation_input_tokens: i64,
    cache_read_input_tokens: i64,
    output_tokens: i64,
    seen: bool,
}

impl UsageAccumulator {
    /// Assembles a [`TokenUsage`] from the split Anthropic usage fields.
    ///
    /// Billing-skew caveat: Anthropic bills `cache_creation_input_tokens` at
    /// 1.25x and `cache_read_input_tokens` at 0.1x the base input rate. We sum
    /// all three into `input_tokens` so the figure reflects *context volume*
    /// (what auto-compaction thresholds care about), not billed cost — the
    /// derived totals therefore do not correspond to dollars in either
    /// direction. `total_tokens` does not exist on the wire and is
    /// synthesized. Anthropic does not report a reasoning-token split, so
    /// `reasoning_output_tokens` stays 0.
    fn to_token_usage(&self) -> Option<TokenUsage> {
        if !self.seen {
            return None;
        }
        let input_total =
            self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens;
        Some(TokenUsage {
            input_tokens: input_total,
            cached_input_tokens: self.cache_read_input_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: 0,
            total_tokens: input_total + self.output_tokens,
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct MessagesSseState {
    response_id: Option<String>,
    usage: UsageAccumulator,
    stop_reason: Option<String>,
    blocks: HashMap<u64, PartialBlock>,
    /// Block indexes in `content_block_start` arrival order. Invariant: while
    /// non-empty, the front block is the active one (its `OutputItemAdded`
    /// has been emitted and its `OutputItemDone` has not).
    order: VecDeque<u64>,
    /// Names of the freeform tools the request advertised; see
    /// [`FREEFORM_INPUT_PROPERTY`].
    freeform_tool_names: HashSet<String>,
}

impl MessagesSseState {
    pub(crate) fn new(freeform_tool_names: Vec<String>) -> Self {
        Self {
            freeform_tool_names: freeform_tool_names.into_iter().collect(),
            ..Self::default()
        }
    }

    pub(crate) fn handle_event(
        &mut self,
        event: MessagesStreamEvent,
    ) -> Result<Vec<ResponseEvent>, MessagesEventError> {
        let mut out = Vec::new();
        match event {
            MessagesStreamEvent::MessageStart { message } => {
                self.response_id = Some(message.id);
                if let Some(usage) = message.usage {
                    self.usage.seen = true;
                    self.usage.input_tokens = usage.input_tokens;
                    self.usage.cache_creation_input_tokens =
                        usage.cache_creation_input_tokens.unwrap_or(0);
                    self.usage.cache_read_input_tokens = usage.cache_read_input_tokens.unwrap_or(0);
                    self.usage.output_tokens = usage.output_tokens.unwrap_or(0);
                }
                out.push(ResponseEvent::Created);
            }
            MessagesStreamEvent::ContentBlockStart {
                index,
                content_block,
            } => {
                self.on_block_start(index, content_block, &mut out);
            }
            MessagesStreamEvent::ContentBlockDelta { index, delta } => {
                self.on_block_delta(index, delta, &mut out);
            }
            MessagesStreamEvent::ContentBlockStop { index } => {
                self.on_block_stop(index, &mut out);
            }
            MessagesStreamEvent::MessageDelta { delta, usage } => {
                if let Some(delta) = delta
                    && let Some(stop_reason) = delta.stop_reason
                {
                    self.stop_reason = Some(stop_reason);
                }
                if let Some(usage) = usage
                    && let Some(output_tokens) = usage.output_tokens
                {
                    // Cumulative on the wire: assign, do not add.
                    self.usage.seen = true;
                    self.usage.output_tokens = output_tokens;
                }
            }
            MessagesStreamEvent::MessageStop => {
                // Defensive: a well-formed stream has closed every block by
                // now; flush any stragglers so `Completed` is never emitted
                // with an item still open.
                while let Some(&index) = self.order.front() {
                    debug!("message_stop with content block {index} still open; flushing");
                    self.on_block_stop(index, &mut out);
                }
                // Truncation is not a turn end: emitting `Completed` here would
                // make core persist a half-written answer as the finished turn.
                // The Responses adapter treats `response.incomplete` the same
                // way, so truncation surfaces and the stream-retry layer decides.
                if self.stop_reason.as_deref() == Some("max_tokens") {
                    return Err(MessagesEventError::Api(ApiError::Stream(
                        "messages response truncated at max_tokens".to_string(),
                    )));
                }
                out.push(ResponseEvent::Completed {
                    response_id: self.response_id.clone().unwrap_or_default(),
                    token_usage: self.usage.to_token_usage(),
                    end_turn: end_turn_from_stop_reason(self.stop_reason.as_deref()),
                });
            }
            MessagesStreamEvent::Ping => {}
            MessagesStreamEvent::Error { error } => {
                return Err(MessagesEventError::Api(api_error_from_messages_error(
                    &error.kind,
                    error.message,
                    /*delay*/ None,
                )));
            }
            MessagesStreamEvent::Unknown => {
                trace!("unhandled messages event");
            }
        }
        Ok(out)
    }

    fn on_block_start(
        &mut self,
        index: u64,
        content_block: ContentBlockStart,
        out: &mut Vec<ResponseEvent>,
    ) {
        let kind = match content_block {
            ContentBlockStart::Text { text } => PartialBlockKind::Text { text },
            ContentBlockStart::Thinking {
                thinking,
                signature,
            } => PartialBlockKind::Thinking {
                thinking,
                signature,
            },
            ContentBlockStart::RedactedThinking { data } => {
                PartialBlockKind::RedactedThinking { data }
            }
            ContentBlockStart::ToolUse { id, name, input } => PartialBlockKind::ToolUse {
                freeform: self.freeform_tool_names.contains(&name),
                id,
                name,
                json_fragments: String::new(),
                start_input: input,
            },
            ContentBlockStart::Unknown => {
                debug!("ignoring unsupported content block at index {index}");
                PartialBlockKind::Unknown
            }
        };
        if self
            .blocks
            .insert(
                index,
                PartialBlock {
                    kind,
                    buffered: Vec::new(),
                    stopped: false,
                },
            )
            .is_some()
        {
            debug!("duplicate content_block_start for index {index}; replacing");
        } else {
            self.order.push_back(index);
        }
        // If this is the only open block it becomes active immediately.
        if self.order.front() == Some(&index)
            && let Some(block) = self.blocks.get(&index)
        {
            out.extend(added_event_for(&block.kind));
        }
    }

    fn on_block_delta(
        &mut self,
        index: u64,
        delta: ContentBlockDelta,
        out: &mut Vec<ResponseEvent>,
    ) {
        let is_active = self.order.front() == Some(&index);
        let Some(block) = self.blocks.get_mut(&index) else {
            debug!("content_block_delta for unknown index {index}");
            return;
        };
        match (&mut block.kind, delta) {
            (PartialBlockKind::Text { text }, ContentBlockDelta::TextDelta { text: chunk }) => {
                text.push_str(&chunk);
                if is_active {
                    out.push(ResponseEvent::OutputTextDelta(chunk));
                } else {
                    block.buffered.push(BufferedDelta::Text(chunk));
                }
            }
            (
                PartialBlockKind::Thinking { thinking, .. },
                ContentBlockDelta::ThinkingDelta { thinking: chunk },
            ) => {
                thinking.push_str(&chunk);
                if is_active {
                    out.push(ResponseEvent::ReasoningContentDelta {
                        delta: chunk,
                        content_index: 0,
                    });
                } else {
                    block.buffered.push(BufferedDelta::Thinking(chunk));
                }
            }
            (
                PartialBlockKind::Thinking { signature, .. },
                ContentBlockDelta::SignatureDelta { signature: chunk },
            ) => {
                // `signature_delta` has no `ResponseEvent` home; it is only
                // captured for the round-trip envelope at block stop.
                signature.push_str(&chunk);
            }
            (
                PartialBlockKind::ToolUse {
                    id, json_fragments, ..
                },
                ContentBlockDelta::InputJsonDelta { partial_json },
            ) => {
                json_fragments.push_str(&partial_json);
                if is_active {
                    out.push(ResponseEvent::ToolCallInputDelta {
                        item_id: id.clone(),
                        call_id: Some(id.clone()),
                        delta: partial_json,
                    });
                } else {
                    block.buffered.push(BufferedDelta::ToolJson(partial_json));
                }
            }
            (PartialBlockKind::Unknown, _) | (_, ContentBlockDelta::Unknown) => {}
            (kind, delta) => {
                debug!("mismatched content_block_delta {delta:?} for block kind {kind:?}");
            }
        }
    }

    fn on_block_stop(&mut self, index: u64, out: &mut Vec<ResponseEvent>) {
        let Some(block) = self.blocks.get_mut(&index) else {
            debug!("content_block_stop for unknown index {index}");
            return;
        };
        block.stopped = true;
        if self.order.front() != Some(&index) {
            // Buffered block: it is finalized when it reaches the front.
            return;
        }
        self.finish_front_block(out);
        self.drain_ready(out);
    }

    /// Emits `OutputItemDone` for the front (active) block and removes it.
    fn finish_front_block(&mut self, out: &mut Vec<ResponseEvent>) {
        let Some(index) = self.order.pop_front() else {
            return;
        };
        let Some(block) = self.blocks.remove(&index) else {
            return;
        };
        out.extend(done_event_for(block.kind));
    }

    /// Promotes buffered blocks to active, flushing complete triples for any
    /// that already stopped while buffered.
    fn drain_ready(&mut self, out: &mut Vec<ResponseEvent>) {
        while let Some(&index) = self.order.front() {
            let Some(block) = self.blocks.get_mut(&index) else {
                self.order.pop_front();
                continue;
            };
            out.extend(added_event_for(&block.kind));
            let buffered = std::mem::take(&mut block.buffered);
            let stopped = block.stopped;
            for delta in buffered {
                out.extend(event_for_buffered_delta(&block.kind, delta));
            }
            if stopped {
                self.finish_front_block(out);
            } else {
                // Front block is now active and still streaming.
                break;
            }
        }
    }
}

/// `OutputItemAdded` shell for a block; unknown blocks emit nothing.
fn added_event_for(kind: &PartialBlockKind) -> Option<ResponseEvent> {
    let item = match kind {
        PartialBlockKind::Text { .. } => ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: Vec::new(),
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        PartialBlockKind::Thinking { .. } | PartialBlockKind::RedactedThinking { .. } => {
            ResponseItem::Reasoning {
                id: None,
                summary: Vec::new(),
                content: None,
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            }
        }
        PartialBlockKind::ToolUse { id, name, .. } => ResponseItem::FunctionCall {
            id: None,
            name: name.clone(),
            namespace: None,
            arguments: String::new(),
            call_id: id.clone(),
            internal_chat_message_metadata_passthrough: None,
        },
        PartialBlockKind::Unknown => return None,
    };
    Some(ResponseEvent::OutputItemAdded(item))
}

/// `OutputItemDone` with the fully assembled item; unknown blocks emit nothing.
fn done_event_for(kind: PartialBlockKind) -> Option<ResponseEvent> {
    let item = match kind {
        PartialBlockKind::Text { text } => ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText { text }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        PartialBlockKind::Thinking {
            thinking,
            signature,
        } => ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: thinking.clone(),
            }]),
            encrypted_content: Some(thinking_envelope(&thinking, &signature)),
            internal_chat_message_metadata_passthrough: None,
        },
        PartialBlockKind::RedactedThinking { data } => ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: None,
            encrypted_content: Some(redacted_thinking_envelope(&data)),
            internal_chat_message_metadata_passthrough: None,
        },
        PartialBlockKind::ToolUse {
            id,
            name,
            json_fragments,
            start_input,
            freeform,
        } => {
            let arguments = stringify_tool_arguments(&json_fragments, &start_input);
            if freeform {
                // A freeform tool was advertised as a one-property object tool,
                // so unwrap that property back into the raw payload text: core
                // routes freeform handlers from `CustomToolCall` only.
                //
                // If the model did not honor that shape, still emit
                // `CustomToolCall` with the raw arguments as the payload. A
                // `FunctionCall` here would be structurally undispatchable —
                // the freeform handler rejects non-custom payloads outright —
                // so the turn would die on a fatal incompatible-payload error.
                // Handing the handler the raw text instead yields at worst a
                // recoverable parse error the model can retry.
                let input =
                    freeform_input_from_arguments(&arguments).unwrap_or_else(|| arguments.clone());
                ResponseItem::CustomToolCall {
                    id: None,
                    status: None,
                    call_id: id,
                    name,
                    input,
                    internal_chat_message_metadata_passthrough: None,
                }
            } else {
                ResponseItem::FunctionCall {
                    id: None,
                    name,
                    namespace: None,
                    arguments,
                    call_id: id,
                    internal_chat_message_metadata_passthrough: None,
                }
            }
        }
        PartialBlockKind::Unknown => return None,
    };
    Some(ResponseEvent::OutputItemDone(item))
}

fn event_for_buffered_delta(
    kind: &PartialBlockKind,
    delta: BufferedDelta,
) -> Option<ResponseEvent> {
    match delta {
        BufferedDelta::Text(chunk) => Some(ResponseEvent::OutputTextDelta(chunk)),
        BufferedDelta::Thinking(chunk) => Some(ResponseEvent::ReasoningContentDelta {
            delta: chunk,
            content_index: 0,
        }),
        BufferedDelta::ToolJson(chunk) => {
            let PartialBlockKind::ToolUse { id, .. } = kind else {
                return None;
            };
            Some(ResponseEvent::ToolCallInputDelta {
                item_id: id.clone(),
                call_id: Some(id.clone()),
                delta: chunk,
            })
        }
    }
}

/// Core parses `ResponseItem::FunctionCall::arguments` as a JSON *string*, so
/// the accumulated Anthropic fragments (or the `content_block_start` `input`
/// object) are re-stringified here.
/// Extracts a freeform tool's raw payload from the single-property object the
/// Messages request advertised.
///
/// Returns `None` when the model did not honor that shape (missing property,
/// non-string value, extra keys), in which case the caller keeps the call as a
/// `FunctionCall` rather than fabricating a payload.
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

fn stringify_tool_arguments(json_fragments: &str, start_input: &Value) -> String {
    if !json_fragments.trim().is_empty() {
        return match serde_json::from_str::<Value>(json_fragments) {
            Ok(value) => value.to_string(),
            Err(err) => {
                debug!("tool_use input fragments are not valid JSON ({err}); passing through raw");
                json_fragments.to_string()
            }
        };
    }
    start_input.to_string()
}

/// Round-trip envelope stored in `ResponseItem::Reasoning::encrypted_content`.
///
/// Anthropic rejects replayed `thinking` blocks whose signature does not match
/// the text, so both halves travel together; the request builder decodes this
/// envelope back into a `thinking` block.
fn thinking_envelope(thinking: &str, signature: &str) -> String {
    let envelope = serde_json::json!({
        "thinking": thinking,
        "signature": signature,
    });
    base64::engine::general_purpose::STANDARD.encode(envelope.to_string())
}

fn redacted_thinking_envelope(data: &str) -> String {
    let envelope = serde_json::json!({ "redacted": data });
    base64::engine::general_purpose::STANDARD.encode(envelope.to_string())
}

fn end_turn_from_stop_reason(stop_reason: Option<&str>) -> Option<bool> {
    match stop_reason {
        Some("tool_use") => Some(false),
        Some("end_turn") | Some("stop_sequence") => Some(true),
        // Unreachable in practice: `message_stop` fails the stream on
        // max_tokens before consulting this mapping (truncation is not a turn
        // end). Kept exhaustive so a future caller cannot silently accept it.
        Some("max_tokens") => Some(true),
        Some(other) => {
            debug!("unknown messages stop_reason: {other}");
            None
        }
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// Maps an Anthropic error payload to an [`ApiError`].
///
/// Retryability is decided here: `overloaded_error`/`rate_limit_error` (and
/// `api_error`, Anthropic's 5xx) must land on `ApiError::Retryable` — mapping
/// them to `ServerOverloaded` would make them fatal (`ForallErr::is_retryable`
/// excludes it), and the transport layer will not help because `retry_429` is
/// hardcoded off. `authentication_error` is surfaced as the HTTP-401 transport
/// error shape the core retry loop matches on for token refresh.
pub(crate) fn api_error_from_messages_error(
    kind: &str,
    message: String,
    delay: Option<Duration>,
) -> ApiError {
    match kind {
        "overloaded_error" | "rate_limit_error" | "api_error" => {
            ApiError::Retryable { message, delay }
        }
        "invalid_request_error" | "not_found_error" | "permission_error" => {
            ApiError::InvalidRequest { message }
        }
        "authentication_error" => ApiError::Transport(TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: None,
            headers: None,
            body: Some(message),
        }),
        _ => ApiError::Stream(message),
    }
}

/// Rewrites HTTP-level errors from the Messages endpoint using the Anthropic
/// error body (`{"type": "error", "error": {"type": ..., "message": ...}}`).
///
/// 401s pass through untouched so the core unauthorized-retry loop keeps
/// matching on `ApiError::Transport(TransportError::Http { status: 401, .. })`.
/// Unrecognized bodies also pass through unchanged.
pub(crate) fn map_messages_http_error(error: ApiError) -> ApiError {
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
    let Some(envelope) = body
        .as_deref()
        .and_then(|body| serde_json::from_str::<MessagesErrorEnvelope>(body).ok())
    else {
        return original(status, url, headers, body);
    };
    match envelope.error.kind.as_str() {
        "overloaded_error" | "rate_limit_error" | "api_error" => {
            let delay = retry_after_from_headers(headers.as_ref());
            ApiError::Retryable {
                message: envelope.error.message,
                delay,
            }
        }
        "invalid_request_error" | "not_found_error" | "permission_error" => {
            ApiError::InvalidRequest {
                message: envelope.error.message,
            }
        }
        _ => original(status, url, headers, body),
    }
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

// ---------------------------------------------------------------------------
// Rate-limit headers
// ---------------------------------------------------------------------------

/// Maps `anthropic-ratelimit-*` response headers to [`RateLimitSnapshot`]s.
///
/// Only the trivially derivable subset is emitted: `used_percent` from
/// `limit`/`remaining` and `resets_at` from the RFC 3339 reset header. Window
/// lengths are not reported by Anthropic and stay `None`; families without
/// both `limit` and `remaining` are skipped rather than fabricated.
pub(crate) fn parse_anthropic_rate_limits(headers: &HeaderMap) -> Vec<RateLimitSnapshot> {
    ["requests", "tokens"]
        .into_iter()
        .filter_map(|family| parse_anthropic_rate_limit_family(headers, family))
        .collect()
}

fn parse_anthropic_rate_limit_family(
    headers: &HeaderMap,
    family: &str,
) -> Option<RateLimitSnapshot> {
    let limit = parse_header_i64(headers, &format!("anthropic-ratelimit-{family}-limit"))?;
    let remaining = parse_header_i64(headers, &format!("anthropic-ratelimit-{family}-remaining"))?;
    if limit <= 0 {
        return None;
    }
    let used = (limit - remaining).max(0);
    #[allow(clippy::cast_precision_loss)]
    let used_percent = (used as f64 / limit as f64 * 100.0).clamp(0.0, 100.0);
    let resets_at = headers
        .get(format!("anthropic-ratelimit-{family}-reset"))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value.trim()).ok())
        .map(|dt| dt.timestamp());

    Some(RateLimitSnapshot {
        limit_id: Some(format!("anthropic_{family}")),
        limit_name: None,
        primary: Some(RateLimitWindow {
            used_percent,
            window_minutes: None,
            resets_at,
        }),
        secondary: None,
        credits: None,
        individual_limit: None,
        plan_type: None,
        rate_limit_reached_type: None,
    })
}

fn parse_header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok()
}

#[cfg(test)]
mod tests {
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

    /// Feeds a recorded Anthropic SSE transcript (one JSON value per event)
    /// through the state machine and returns every emitted event/error.
    async fn run_messages_sse(
        events: Vec<serde_json::Value>,
    ) -> Vec<Result<ResponseEvent, ApiError>> {
        let mut body = String::new();
        for e in events {
            let kind = e
                .get("type")
                .and_then(|v| v.as_str())
                .expect("fixture event missing type");
            body.push_str(&format!("event: {kind}\ndata: {e}\n\n"));
        }
        run_messages_sse_bytes(body).await
    }

    /// Same as [`run_messages_sse`], but declares `freeform_tool_names` so
    /// their `tool_use` blocks are unwrapped into `CustomToolCall`.
    async fn run_messages_sse_with_freeform(
        events: Vec<serde_json::Value>,
        freeform_tool_names: Vec<String>,
    ) -> Vec<Result<ResponseEvent, ApiError>> {
        let mut body = String::new();
        for e in events {
            let kind = e
                .get("type")
                .and_then(|v| v.as_str())
                .expect("fixture event missing type");
            body.push_str(&format!("event: {kind}\ndata: {e}\n\n"));
        }
        run_messages_sse_bytes_with_freeform(body, freeform_tool_names).await
    }

    async fn run_messages_sse_bytes(body: String) -> Vec<Result<ResponseEvent, ApiError>> {
        run_messages_sse_bytes_with_freeform(body, Vec::new()).await
    }

    async fn run_messages_sse_bytes_with_freeform(
        body: String,
        freeform_tool_names: Vec<String>,
    ) -> Vec<Result<ResponseEvent, ApiError>> {
        let stream = ReaderStream::new(std::io::Cursor::new(body))
            .map_err(|err| TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
        tokio::spawn(process_messages_sse(
            Box::pin(stream),
            tx,
            idle_timeout(),
            /*telemetry*/ None,
            freeform_tool_names,
        ));

        let mut out = Vec::new();
        while let Some(ev) = rx.recv().await {
            out.push(ev);
        }
        out
    }

    fn message_start(id: &str, usage: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "message_start",
            "message": {
                "id": id,
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5",
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage
            }
        })
    }

    fn decode_envelope(encrypted_content: &str) -> serde_json::Value {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encrypted_content)
            .expect("envelope should be valid base64");
        serde_json::from_slice(&decoded).expect("envelope should be JSON")
    }

    #[tokio::test]
    async fn text_only_turn_emits_triple_and_completed() {
        let events = run_messages_sse(vec![
            message_start("msg_1", json!({"input_tokens": 12, "output_tokens": 1})),
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
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 9}
            }),
            json!({"type": "message_stop"}),
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
                assert_eq!(response_id, "msg_1");
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
    async fn thinking_then_text_builds_reasoning_envelope() {
        let events = run_messages_sse(vec![
            message_start("msg_2", json!({"input_tokens": 5, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Let me think"}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": " harder"}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "signature_delta", "signature": "sig-abc"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "text", "text": ""}
            }),
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "text_delta", "text": "Answer"}
            }),
            json!({"type": "content_block_stop", "index": 1}),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": {"output_tokens": 4}
            }),
            json!({"type": "message_stop"}),
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
                summary,
                content,
                encrypted_content: Some(encrypted_content),
                ..
            })) => {
                assert!(summary.is_empty());
                assert_eq!(
                    content,
                    &Some(vec![ReasoningItemContent::ReasoningText {
                        text: "Let me think harder".to_string(),
                    }])
                );
                assert_eq!(
                    decode_envelope(encrypted_content),
                    json!({"thinking": "Let me think harder", "signature": "sig-abc"})
                );
            }
            other => panic!("unexpected reasoning done event: {other:?}"),
        }
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(&events[6], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Answer");
        assert_matches!(
            &events[7],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { .. }))
        );
        assert_matches!(
            &events[8],
            Ok(ResponseEvent::Completed {
                end_turn: Some(true),
                ..
            })
        );
    }

    #[tokio::test]
    async fn redacted_thinking_produces_redacted_envelope() {
        let events = run_messages_sse(vec![
            message_start("msg_r", json!({"input_tokens": 1, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "redacted_thinking", "data": "opaque-bytes"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_eq!(events.len(), 4);
        match &events[2] {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                content: None,
                encrypted_content: Some(encrypted_content),
                ..
            })) => {
                assert_eq!(
                    decode_envelope(encrypted_content),
                    json!({"redacted": "opaque-bytes"})
                );
            }
            other => panic!("unexpected redacted thinking event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_use_streams_deltas_and_restringifies_arguments() {
        let events = run_messages_sse(vec![
            message_start("msg_3", json!({"input_tokens": 8, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_1", "name": "shell", "input": {}}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "\"/tmp\","}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "\"recursive\":true}"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 15}
            }),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_eq!(events.len(), 7);
        assert_matches!(&events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            })) if name == "shell" && arguments.is_empty() && call_id == "toolu_1"
        );
        for (i, expected) in ["{\"path\":", "\"/tmp\",", "\"recursive\":true}"]
            .iter()
            .enumerate()
        {
            assert_matches!(
                &events[2 + i],
                Ok(ResponseEvent::ToolCallInputDelta {
                    item_id,
                    call_id: Some(call_id),
                    delta,
                }) if item_id == "toolu_1" && call_id == "toolu_1" && delta == expected
            );
        }
        match &events[5] {
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            })) => {
                assert_eq!(name, "shell");
                assert_eq!(call_id, "toolu_1");
                let parsed: Value =
                    serde_json::from_str(arguments).expect("arguments should be a JSON string");
                assert_eq!(parsed, json!({"path": "/tmp", "recursive": true}));
            }
            other => panic!("unexpected function call done event: {other:?}"),
        }
        assert_matches!(
            &events[6],
            Ok(ResponseEvent::Completed {
                end_turn: Some(false),
                ..
            })
        );
    }

    #[tokio::test]
    async fn tool_use_without_deltas_stringifies_start_input() {
        let events = run_messages_sse(vec![
            message_start("msg_4", json!({"input_tokens": 1, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "tool_use", "id": "toolu_2", "name": "noop", "input": {}}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 2}
            }),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_matches!(
            &events[2],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }))
                if arguments == "{}"
        );
    }

    #[tokio::test]
    async fn interleaved_blocks_are_buffered_into_sequential_triples() {
        let events = run_messages_sse(vec![
            message_start("msg_5", json!({"input_tokens": 10, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "tool_use", "id": "toolu_9", "name": "shell", "input": {}}
            }),
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":\"ls\"}"}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "Hello"}
            }),
            json!({"type": "content_block_stop", "index": 1}),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": " world"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 7}
            }),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_eq!(events.len(), 9);
        assert_matches!(&events[0], Ok(ResponseEvent::Created));
        // Block 0 streams live while block 1 is buffered.
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message { .. }))
        );
        assert_matches!(&events[2], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Hello");
        assert_matches!(&events[3], Ok(ResponseEvent::OutputTextDelta(delta)) if delta == " world");
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "Hello world".to_string() }]
        );
        // Block 1 is flushed as a complete triple after block 0 closes.
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::OutputItemAdded(ResponseItem::FunctionCall {
                arguments,
                call_id,
                ..
            })) if arguments.is_empty() && call_id == "toolu_9"
        );
        assert_matches!(
            &events[6],
            Ok(ResponseEvent::ToolCallInputDelta {
                item_id,
                call_id: Some(call_id),
                delta,
            }) if item_id == "toolu_9" && call_id == "toolu_9" && delta == "{\"cmd\":\"ls\"}"
        );
        assert_matches!(
            &events[7],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }))
                if arguments == "{\"cmd\":\"ls\"}"
        );
        assert_matches!(
            &events[8],
            Ok(ResponseEvent::Completed {
                end_turn: Some(false),
                ..
            })
        );
    }

    #[tokio::test]
    async fn usage_is_assembled_across_message_start_and_message_delta() {
        let events = run_messages_sse(vec![
            message_start(
                "msg_6",
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
        ])
        .await;

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
    async fn max_tokens_surfaces_truncation_instead_of_completing() {
        // Behavior change (PR #80 review, applied here too because the shipped
        // Messages adapter had the identical flaw): a truncated response used to
        // arrive as Completed, so core persisted a half-written answer as the
        // finished turn. It now fails the stream, matching how the Responses
        // adapter treats `response.incomplete`.
        let events = run_messages_sse(vec![
            message_start("msg_trunc", json!({"input_tokens": 5, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "half an ans"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "max_tokens"},
                "usage": {"output_tokens": 9}
            }),
            json!({"type": "message_stop"}),
        ])
        .await;

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
    async fn stop_reason_matrix_maps_to_end_turn() {
        let cases: Vec<(Option<&str>, Option<bool>)> = vec![
            (Some("end_turn"), Some(true)),
            (Some("stop_sequence"), Some(true)),
            // max_tokens is deliberately absent: truncation fails the stream
            // rather than completing (see
            // max_tokens_surfaces_truncation_instead_of_completing).
            (Some("tool_use"), Some(false)),
            (Some("pause_turn"), None),
            (None, None),
        ];

        for (stop_reason, expected) in cases {
            let mut fixture = vec![message_start(
                "msg_7",
                json!({"input_tokens": 1, "output_tokens": 0}),
            )];
            if let Some(stop_reason) = stop_reason {
                fixture.push(json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": stop_reason},
                    "usage": {"output_tokens": 1}
                }));
            }
            fixture.push(json!({"type": "message_stop"}));

            let events = run_messages_sse(fixture).await;
            match events.last() {
                Some(Ok(ResponseEvent::Completed { end_turn, .. })) => {
                    assert_eq!(*end_turn, expected, "stop_reason case {stop_reason:?}");
                }
                other => panic!("unexpected final event for {stop_reason:?}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn overloaded_error_event_is_retryable() {
        let events = run_messages_sse(vec![
            message_start("msg_8", json!({"input_tokens": 1, "output_tokens": 0})),
            json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "Overloaded"}
            }),
        ])
        .await;

        assert_eq!(events.len(), 2);
        assert_matches!(&events[0], Ok(ResponseEvent::Created));
        match &events[1] {
            Err(ApiError::Retryable { message, delay }) => {
                assert_eq!(message, "Overloaded");
                assert_eq!(*delay, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn authentication_error_event_maps_to_http_unauthorized() {
        let events = run_messages_sse(vec![json!({
            "type": "error",
            "error": {"type": "authentication_error", "message": "invalid x-api-key"}
        })])
        .await;

        assert_eq!(events.len(), 1);
        assert_matches!(
            &events[0],
            Err(ApiError::Transport(TransportError::Http { status, .. }))
                if *status == StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn stream_end_without_message_stop_is_an_error() {
        let events = run_messages_sse(vec![
            message_start("msg_9", json!({"input_tokens": 1, "output_tokens": 0})),
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "partial"}
            }),
            json!({"type": "content_block_stop", "index": 0}),
        ])
        .await;

        assert_eq!(events.len(), 5);
        match &events[4] {
            Err(ApiError::Stream(msg)) => {
                assert_eq!(msg, "stream closed before response.completed");
            }
            other => panic!("unexpected final event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn ping_and_unknown_events_are_ignored() {
        let events = run_messages_sse(vec![
            message_start("msg_10", json!({"input_tokens": 1, "output_tokens": 0})),
            json!({"type": "ping"}),
            json!({"type": "content_block_heartbeat"}),
            json!({"type": "message_stop"}),
        ])
        .await;

        assert_eq!(events.len(), 2);
        assert_matches!(&events[0], Ok(ResponseEvent::Created));
        assert_matches!(&events[1], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn spawn_messages_stream_emits_request_id_and_rate_limits() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ANTHROPIC_REQUEST_ID_HEADER,
            HeaderValue::from_static("req_abc"),
        );
        headers.insert(
            "anthropic-ratelimit-requests-limit",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "anthropic-ratelimit-requests-remaining",
            HeaderValue::from_static("75"),
        );
        headers.insert(
            "anthropic-ratelimit-requests-reset",
            HeaderValue::from_static("2026-01-01T00:00:00Z"),
        );
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_h\",\"usage\":{\"input_tokens\":1}}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let bytes = futures::stream::iter(vec![Ok(bytes::Bytes::from_static(body.as_bytes()))]);
        let stream_response = StreamResponse {
            status: StatusCode::OK,
            headers,
            bytes: Box::pin(bytes),
        };

        let mut stream = spawn_messages_stream(
            stream_response,
            idle_timeout(),
            /*telemetry*/ None,
            /*turn_state*/ None,
            /*freeform_tool_names*/ Vec::new(),
        );
        assert_eq!(stream.upstream_request_id.as_deref(), Some("req_abc"));

        let mut events = Vec::new();
        while let Some(event) = stream.rx_event.recv().await {
            events.push(event.expect("expected ok event"));
        }
        assert_eq!(events.len(), 3);
        match &events[0] {
            ResponseEvent::RateLimits(snapshot) => {
                assert_eq!(snapshot.limit_id.as_deref(), Some("anthropic_requests"));
                let primary = snapshot.primary.as_ref().expect("primary window");
                assert_eq!(primary.used_percent, 25.0);
                assert_eq!(primary.window_minutes, None);
                assert_eq!(primary.resets_at, Some(1_767_225_600));
            }
            other => panic!("expected rate limit event, got {other:?}"),
        }
        assert_matches!(&events[1], ResponseEvent::Created);
        assert_matches!(&events[2], ResponseEvent::Completed { response_id, .. } if response_id == "msg_h");
    }

    #[test]
    fn http_rate_limit_error_maps_to_retryable_with_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::RETRY_AFTER, HeaderValue::from_static("30"));
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::TOO_MANY_REQUESTS,
            url: None,
            headers: Some(headers),
            body: Some(
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"}}"#
                    .to_string(),
            ),
        });

        match map_messages_http_error(error) {
            ApiError::Retryable { message, delay } => {
                assert_eq!(message, "Rate limited");
                assert_eq!(delay, Some(Duration::from_secs(30)));
            }
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn http_overloaded_error_maps_to_retryable() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::from_u16(529).expect("valid status"),
            url: None,
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
                    .to_string(),
            ),
        });

        match map_messages_http_error(error) {
            ApiError::Retryable { message, delay } => {
                assert_eq!(message, "Overloaded");
                assert_eq!(delay, None);
            }
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn http_invalid_request_error_maps_to_invalid_request() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::BAD_REQUEST,
            url: None,
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad prompt"}}"#
                    .to_string(),
            ),
        });

        match map_messages_http_error(error) {
            ApiError::InvalidRequest { message } => assert_eq!(message, "bad prompt"),
            other => panic!("unexpected mapping: {other:?}"),
        }
    }

    #[test]
    fn http_unauthorized_passes_through_for_core_retry_loop() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: None,
            headers: None,
            body: Some(
                r#"{"type":"error","error":{"type":"authentication_error","message":"nope"}}"#
                    .to_string(),
            ),
        });

        assert_matches!(
            map_messages_http_error(error),
            ApiError::Transport(TransportError::Http { status, .. })
                if status == StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn http_unrecognized_body_passes_through() {
        let error = ApiError::Transport(TransportError::Http {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            url: None,
            headers: None,
            body: Some("<html>gateway error</html>".to_string()),
        });

        assert_matches!(
            map_messages_http_error(error),
            ApiError::Transport(TransportError::Http { status, .. })
                if status == StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn rate_limit_headers_without_remaining_are_skipped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "anthropic-ratelimit-tokens-limit",
            HeaderValue::from_static("1000"),
        );

        assert!(parse_anthropic_rate_limits(&headers).is_empty());
    }
    #[tokio::test]
    async fn freeform_tool_use_is_unwrapped_into_custom_tool_call() {
        // Regression (PR #77 review): the bundled Claude catalog advertises
        // apply_patch as a freeform tool. It reaches Anthropic as a
        // single-string-property object tool, so the call must come back as
        // CustomToolCall { input } — the only shape core routes to a freeform
        // handler. A FunctionCall here silently breaks repository editing.
        let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-old\n+new\n*** End Patch";
        let arguments = serde_json::json!({ "input": patch }).to_string();
        let events = run_messages_sse_with_freeform(
            vec![
                message_start("msg_ff", json!({"input_tokens": 5, "output_tokens": 0})),
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
            ],
            vec!["apply_patch".to_string()],
        )
        .await;

        let done = events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(item)) => Some(item),
                _ => None,
            })
            .next()
            .expect("an OutputItemDone for the freeform call");
        match done {
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                ..
            } => {
                assert_eq!(name, "apply_patch");
                assert_eq!(call_id, "toolu_ff");
                assert_eq!(input, patch, "raw payload must survive unwrapping");
            }
            other => panic!("expected CustomToolCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_freeform_payload_still_dispatches_as_custom_tool_call() {
        // Regression (PR #77 review): a freeform tool invoked with the wrong
        // input shape must NOT degrade to FunctionCall — the freeform handler
        // rejects non-custom payloads outright, so that would be a fatal
        // incompatible-payload error. Emitting CustomToolCall with the raw
        // arguments keeps the failure recoverable (the handler can reject the
        // payload and the model can retry).
        let events = run_messages_sse_with_freeform(
            vec![
                message_start("msg_bad", json!({"input_tokens": 5, "output_tokens": 0})),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_bad",
                        "name": "apply_patch",
                        // Wrong shape: extra key and a non-string payload.
                        "input": {"patch": {"nested": true}, "extra": 1}
                    }
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"},
                    "usage": {"output_tokens": 3}
                }),
                json!({"type": "message_stop"}),
            ],
            vec!["apply_patch".to_string()],
        )
        .await;

        let done = events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(item)) => Some(item),
                _ => None,
            })
            .next()
            .expect("an OutputItemDone");
        match done {
            ResponseItem::CustomToolCall { name, input, .. } => {
                assert_eq!(name, "apply_patch");
                assert!(
                    input.contains("nested"),
                    "raw arguments must reach the handler, got {input:?}"
                );
            }
            other => panic!("freeform tools must never degrade to FunctionCall, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn undeclared_tool_stays_a_function_call() {
        // Only tools the request advertised as freeform are unwrapped; an
        // ordinary function tool keeps its JSON arguments.
        let events = run_messages_sse_with_freeform(
            vec![
                message_start("msg_fn", json!({"input_tokens": 5, "output_tokens": 0})),
                json!({
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {
                        "type": "tool_use",
                        "id": "toolu_fn",
                        "name": "shell",
                        "input": {"input": "not-freeform"}
                    }
                }),
                json!({"type": "content_block_stop", "index": 0}),
                json!({
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use"},
                    "usage": {"output_tokens": 2}
                }),
                json!({"type": "message_stop"}),
            ],
            vec!["apply_patch".to_string()],
        )
        .await;

        let done = events
            .iter()
            .filter_map(|event| match event {
                Ok(ResponseEvent::OutputItemDone(item)) => Some(item),
                _ => None,
            })
            .next()
            .expect("an OutputItemDone");
        assert!(
            matches!(done, ResponseItem::FunctionCall { name, .. } if name == "shell"),
            "undeclared tools must stay FunctionCall, got {done:?}"
        );
    }

    #[tokio::test]
    async fn error_event_terminates_stream_without_waiting_for_close() {
        // Regression (PR #77 review): an `error` event followed by silence on an
        // open connection previously waited out the full idle timeout before the
        // retry loop saw anything.
        let body = format!(
            "event: error
data: {}

",
            json!({
                "type": "error",
                "error": {"type": "overloaded_error", "message": "overloaded"}
            })
        );
        let stream = ReaderStream::new(std::io::Cursor::new(body))
            .map_err(|err| TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
        // A long idle timeout: if the adapter waited for stream closure or the
        // idle deadline, this test would time out instead of completing.
        tokio::spawn(process_messages_sse(
            Box::pin(stream),
            tx,
            Duration::from_secs(300),
            /*telemetry*/ None,
            Vec::new(),
        ));

        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("error must surface promptly, not after the idle timeout")
            .expect("an event");
        match first {
            Err(ApiError::Retryable { .. }) => {}
            other => panic!("expected a Retryable error, got {other:?}"),
        }
    }
}
