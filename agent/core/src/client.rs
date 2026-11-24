//! Session- and turn-scoped helpers for talking to model provider APIs.
//!
//! `ModelClient` is intended to live for the lifetime of a Forall session and holds the stable
//! configuration and state needed to talk to a provider (auth, provider selection, conversation id,
//! and transport fallback state).
//!
//! Per-turn settings (model selection, reasoning controls, telemetry context, and turn metadata)
//! are passed explicitly to streaming and unary methods so that the turn lifetime is visible at the
//! call site.
//!
//! A [`ModelClientSession`] is created per turn and is used to stream one or more Responses API
//! requests during that turn. It caches a Responses WebSocket connection (opened lazily) and stores
//! per-turn state such as the `x-forall-turn-state` token used for sticky routing.
//!
//! WebSocket prewarm is a v2-only `response.create` with `generate=false`; it waits for completion
//! so the next request can reuse the same connection and `previous_response_id`.
//!
//! Turn execution performs prewarm as a best-effort step before the first stream request so the
//! subsequent request can reuse the same connection.
//!
//! ## Retry-Budget Tradeoff
//!
//! WebSocket prewarm is treated as the first websocket connection attempt for a turn. If it
//! fails, normal stream retry/fallback logic handles recovery on the same turn.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use forall_api::ApiError;
use forall_api::AuthProvider;
use forall_api::BedrockMessagesClient as ApiBedrockMessagesClient;
use forall_api::BedrockMessagesOptions as ApiBedrockMessagesOptions;
use forall_api::CacheControl;
use forall_api::CompactClient as ApiCompactClient;
use forall_api::CompactionInput as ApiCompactionInput;
use forall_api::Compression;
use forall_api::GeminiClient as ApiGeminiClient;
use forall_api::GeminiContent;
use forall_api::GeminiFunctionCall;
use forall_api::GeminiFunctionCallingConfig;
use forall_api::GeminiFunctionCallingMode;
use forall_api::GeminiFunctionDeclaration;
use forall_api::GeminiFunctionResponse;
use forall_api::GeminiGenerationConfig;
use forall_api::GeminiOptions as ApiGeminiOptions;
use forall_api::GeminiPart;
use forall_api::GeminiRequest;
use forall_api::GeminiRole;
use forall_api::GeminiSystemInstruction;
use forall_api::GeminiThinkingConfig;
use forall_api::GeminiTool;
use forall_api::GeminiToolConfig;
use forall_api::MemoriesClient as ApiMemoriesClient;
use forall_api::MemorySummarizeInput as ApiMemorySummarizeInput;
use forall_api::MemorySummarizeOutput as ApiMemorySummarizeOutput;
use forall_api::MessagesApiRequest;
use forall_api::MessagesClient as ApiMessagesClient;
use forall_api::MessagesContentBlock;
use forall_api::MessagesImageSource;
use forall_api::MessagesMessage;
use forall_api::MessagesOptions as ApiMessagesOptions;
use forall_api::MessagesRole;
use forall_api::MessagesSystemBlock;
use forall_api::MessagesToolChoice;
use forall_api::MessagesToolResultContent;
use forall_api::MessagesToolSpec;
use forall_api::Provider as ApiProvider;
use forall_api::RawMemory as ApiRawMemory;
use forall_api::RealtimeCallClient as ApiRealtimeCallClient;
use forall_api::RealtimeSessionConfig as ApiRealtimeSessionConfig;
use forall_api::Reasoning;
use forall_api::ReasoningContext;
use forall_api::RequestTelemetry;
use forall_api::ReqwestTransport;
use forall_api::ResponseCreateWsRequest;
use forall_api::ResponsesApiRequest;
use forall_api::ResponsesClient as ApiResponsesClient;
use forall_api::ResponsesOptions as ApiResponsesOptions;
use forall_api::ResponsesWebsocketClient as ApiWebSocketResponsesClient;
use forall_api::ResponsesWebsocketConnection as ApiWebSocketConnection;
use forall_api::ResponsesWsRequest;
use forall_api::SharedAuthProvider;
use forall_api::SseTelemetry;
use forall_api::ThinkingConfig;
use forall_api::TransportError;
use forall_api::WebsocketTelemetry;
use forall_api::auth_header_telemetry;
use forall_api::build_session_headers;
use forall_api::create_text_param_for_request;
use forall_api::response_create_client_metadata;
use forall_app_server_protocol::AuthMode;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_login::RefreshTokenError;
use forall_login::UnauthorizedRecovery;
use forall_login::default_client::build_reqwest_client;
use forall_otel::SessionTelemetry;
use forall_otel::current_span_w3c_trace_context;

use base64::Engine as _;
use eventsource_stream::Event;
use eventsource_stream::EventStreamError;
use forall_protocol::ThreadId;
use forall_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use forall_protocol::config_types::Verbosity as VerbosityConfig;
use forall_protocol::models::ContentItem;
use forall_protocol::models::FunctionCallOutputBody;
use forall_protocol::models::FunctionCallOutputContentItem;
use forall_protocol::models::FunctionCallOutputPayload;
use forall_protocol::models::ResponseItem;
use forall_protocol::openai_models::ModelInfo;
use forall_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use forall_protocol::protocol::InternalSessionSource;
use forall_protocol::protocol::SessionSource;
use forall_protocol::protocol::W3cTraceContext;
use forall_rollout_trace::CompactionTraceContext;
use forall_rollout_trace::InferenceTraceAttempt;
use forall_rollout_trace::InferenceTraceContext;
use forall_tools::create_tools_json_for_gemini_api;
use forall_tools::create_tools_json_for_messages_api;
use forall_tools::create_tools_json_for_responses_api;
use futures::StreamExt;
use http::HeaderMap as ApiHeaderMap;
use http::HeaderValue;
use http::StatusCode as HttpStatusCode;
use reqwest::StatusCode;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::oneshot::error::TryRecvError;
use tokio_tungstenite::tungstenite::Error;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use tracing::trace;
use tracing::warn;

use crate::attestation::AttestationContext;
use crate::attestation::AttestationProvider;
use crate::attestation::X_OAI_ATTESTATION_HEADER;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::client_common::ResponseStream;
use crate::feedback_tags;
use crate::responses_metadata::ForallResponsesMetadata;
use crate::responses_metadata::subagent_header_value;
use crate::util::emit_feedback_auth_recovery_tags;
use forall_feedback::FeedbackRequestTags;
use forall_feedback::emit_feedback_request_tags_with_auth_env;
use forall_login::auth_env_telemetry::AuthEnvTelemetry;
use forall_login::auth_env_telemetry::collect_auth_env_telemetry;
use forall_model_provider::SharedModelProvider;
use forall_model_provider::create_model_provider;
#[cfg(test)]
use forall_model_provider_info::DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS;
use forall_model_provider_info::ModelProviderInfo;
use forall_model_provider_info::WireApi;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result;
use forall_response_debug_context::extract_response_debug_context;
use forall_response_debug_context::extract_response_debug_context_from_api_error;
use forall_response_debug_context::telemetry_api_error_message;
use forall_response_debug_context::telemetry_transport_error_message;

pub const OPENAI_BETA_HEADER: &str = "OpenAI-Beta";
pub const X_CODEX_INSTALLATION_ID_HEADER: &str = "x-forall-installation-id";
pub const X_CODEX_TURN_STATE_HEADER: &str = "x-forall-turn-state";
pub const X_CODEX_TURN_METADATA_HEADER: &str = "x-forall-turn-metadata";
pub const X_CODEX_PARENT_THREAD_ID_HEADER: &str = "x-forall-parent-thread-id";
pub const X_CODEX_WINDOW_ID_HEADER: &str = "x-forall-window-id";
pub const X_OPENAI_MEMGEN_REQUEST_HEADER: &str = "x-openai-memgen-request";
pub const X_OPENAI_SUBAGENT_HEADER: &str = "x-openai-subagent";
pub const X_RESPONSESAPI_INCLUDE_TIMING_METRICS_HEADER: &str =
    "x-responsesapi-include-timing-metrics";
const X_CODEX_WS_STREAM_REQUEST_START_MS_CLIENT_METADATA_KEY: &str =
    "x-forall-ws-stream-request-start-ms";
const WS_REQUEST_HEADER_RESPONSES_LITE_CLIENT_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_forall_responses_lite";
const RESPONSES_WEBSOCKETS_V2_BETA_HEADER_VALUE: &str = "responses_websockets=2026-02-06";
const X_OPENAI_INTERNAL_CODEX_RESPONSES_LITE_HEADER: &str =
    "x-openai-internal-forall-responses-lite";
const RESPONSES_ENDPOINT: &str = "/responses";
const MESSAGES_ENDPOINT: &str = "/messages";
/// Telemetry route label for Bedrock's Anthropic invoke endpoint. The real path
/// is model-scoped (`model/{modelId}/invoke-with-response-stream`); the id is
/// omitted so the label stays a bounded-cardinality route rather than one series
/// per model.
const BEDROCK_MESSAGES_ENDPOINT: &str = "/model/invoke-with-response-stream";
/// Telemetry route label for the Gemini endpoint. The real path is model-scoped
/// (`models/{model}:streamGenerateContent`); the slug is omitted so the label
/// stays a bounded-cardinality route rather than one series per model.
const GEMINI_ENDPOINT: &str = "/models:streamGenerateContent";
const RESPONSES_COMPACT_ENDPOINT: &str = "/responses/compact";
/// `max_tokens` is required by the Messages API. `ModelInfo` carries no
/// max-output-tokens field, so this Messages-specific default applies to every
/// request. It is deliberately larger than the 8_192 non-OpenAI Responses
/// hardcode (which stays untouched): 8k would silently truncate long outputs
/// with `stop_reason: "max_tokens"`, and it must also leave room for the
/// largest extended-thinking budget (24k) plus visible output.
const MESSAGES_DEFAULT_MAX_OUTPUT_TOKENS: u32 = 32_000;
/// Placeholder inserted when translated history would start with an assistant
/// turn (or be empty): the Messages API requires a non-empty `messages` array
/// whose first entry is a `user` turn.
const MESSAGES_PLACEHOLDER_USER_TEXT: &str = "Continue.";
/// `generationConfig.maxOutputTokens` for Gemini. `ModelInfo` carries no
/// max-output-tokens field, so this Gemini-specific default applies to every
/// request instead of the 8_192 non-OpenAI Responses hardcode (which stays
/// untouched). Gemini spends the thinking budget out of this same allowance, so
/// it must leave room for the largest budget (24k) plus visible output; the
/// bundled 2.5 models all accept up to 65_536.
const GEMINI_DEFAULT_MAX_OUTPUT_TOKENS: u32 = 32_768;
/// Placeholder inserted when translated history is empty: Gemini requires a
/// non-empty `contents` array. Unlike the Messages API, Gemini does NOT require
/// the first entry to be a `user` turn, so this is only a last resort.
const GEMINI_PLACEHOLDER_USER_TEXT: &str = "Continue.";
// `/responses/compact` is unary, so the timeout covers the full response rather than one idle
// period between stream events.
const COMPACT_REQUEST_TIMEOUT_IDLE_MULTIPLIER: u32 = 4;
const MEMORIES_SUMMARIZE_ENDPOINT: &str = "/memories/trace_summarize";
#[cfg(test)]
pub(crate) const WEBSOCKET_CONNECT_TIMEOUT: Duration =
    Duration::from_millis(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS);

pub(crate) struct CompactConversationRequestSettings {
    pub(crate) effort: Option<ReasoningEffortConfig>,
    pub(crate) summary: ReasoningSummaryConfig,
    pub(crate) service_tier: Option<String>,
}

fn session_telemetry_for_request(
    session_telemetry: &SessionTelemetry,
    request: &ResponsesApiRequest,
) -> SessionTelemetry {
    session_telemetry.clone().with_inference_request(
        request.service_tier.as_deref(),
        request
            .reasoning
            .as_ref()
            .and_then(|reasoning| reasoning.effort.as_ref()),
    )
}

/// Session-scoped state shared by all [`ModelClient`] clones.
///
/// This is intentionally kept minimal so `ModelClient` does not need to hold a full `Config`. Most
/// configuration is per turn and is passed explicitly to streaming/unary methods.
#[derive(Debug)]
struct ModelClientState {
    thread_id: ThreadId,
    provider: SharedModelProvider,
    auth_env_telemetry: AuthEnvTelemetry,
    session_source: SessionSource,
    model_verbosity: Option<VerbosityConfig>,
    enable_request_compression: bool,
    include_timing_metrics: bool,
    beta_features_header: Option<String>,
    item_ids_enabled: bool,
    include_attestation: bool,
    attestation_provider: Option<Arc<dyn AttestationProvider>>,
    disable_websockets: AtomicBool,
    cached_websocket_session: StdMutex<WebsocketSession>,
}

/// Resolved API client setup for a single request attempt.
///
/// Keeping this as a single bundle ensures prewarm and normal request paths
/// share the same auth/provider setup flow.
struct CurrentClientSetup {
    auth: Option<ForallAuth>,
    api_provider: ApiProvider,
    api_auth: SharedAuthProvider,
}

#[derive(Clone, Copy)]
struct RequestRouteTelemetry {
    endpoint: &'static str,
}

impl RequestRouteTelemetry {
    fn for_endpoint(endpoint: &'static str) -> Self {
        Self { endpoint }
    }
}

/// A session-scoped client for model-provider API calls.
///
/// This holds configuration and state that should be shared across turns within a Forall session
/// (auth, provider selection, thread id, and transport fallback state).
///
/// WebSocket fallback is session-scoped: once a turn activates the HTTP fallback, subsequent turns
/// will also use HTTP for the remainder of the session.
///
/// Turn-scoped settings (model selection, reasoning controls, telemetry context, and turn
/// metadata) are passed explicitly to the relevant methods to keep turn lifetime visible at the
/// call site.
#[derive(Debug, Clone)]
pub struct ModelClient {
    state: Arc<ModelClientState>,
    prompt_cache_key_override: Option<String>,
}

/// A turn-scoped streaming session created from a [`ModelClient`].
///
/// The session establishes a Responses WebSocket connection lazily and reuses it across multiple
/// requests within the turn. It also caches per-turn state:
///
/// - The last full request, so subsequent calls can reuse incremental websocket request payloads
///   only when the current request is an incremental extension of the previous one.
/// - The `x-forall-turn-state` sticky-routing token, which must be replayed for all requests within
///   the same turn.
///
/// Create a fresh `ModelClientSession` for each Forall turn. Reusing it across turns would replay
/// the previous turn's sticky-routing token into the next turn, which violates the client/server
/// contract and can cause routing bugs.
pub struct ModelClientSession {
    client: ModelClient,
    websocket_session: WebsocketSession,
    /// Turn state for sticky routing.
    ///
    /// This is an `OnceLock` that stores the turn state value received from the server
    /// on turn start via the `x-forall-turn-state` response header. Once set, this value
    /// should be sent back to the server in the `x-forall-turn-state` request header for
    /// all subsequent requests within the same turn to maintain sticky routing.
    ///
    /// This is a contract between the client and server: we receive it at turn start,
    /// keep sending it unchanged between turn requests (e.g., for retries, incremental
    /// appends, or continuation requests), and must not send it between different turns.
    turn_state: Arc<OnceLock<String>>,
}

#[derive(Debug, Clone)]
struct LastResponse {
    response_id: String,
    items_added: Vec<ResponseItem>,
}

#[derive(Debug, Default)]
struct WebsocketSession {
    connection: Option<ApiWebSocketConnection>,
    last_request: Option<ResponsesApiRequest>,
    last_response_rx: Option<oneshot::Receiver<LastResponse>>,
    last_response_from_untraced_warmup: bool,
    connection_reused: StdMutex<bool>,
}

// This is intentionally not a `PartialEq` implementation: request equality includes `input` and
// `client_metadata`, while websocket reuse compares the input separately and ignores metadata.
// Keep the destructuring exhaustive so new request fields require an explicit reuse decision.
fn responses_request_properties_match(
    previous: &ResponsesApiRequest,
    current: &ResponsesApiRequest,
) -> bool {
    let ResponsesApiRequest {
        model: previous_model,
        instructions: previous_instructions,
        input: _,
        tools: previous_tools,
        tool_choice: previous_tool_choice,
        parallel_tool_calls: previous_parallel_tool_calls,
        reasoning: previous_reasoning,
        store: previous_store,
        stream: previous_stream,
        include: previous_include,
        service_tier: previous_service_tier,
        prompt_cache_key: previous_prompt_cache_key,
        text: previous_text,
        client_metadata: _,
        max_output_tokens: previous_max_output_tokens,
    } = previous;
    let ResponsesApiRequest {
        model: current_model,
        instructions: current_instructions,
        input: _,
        tools: current_tools,
        tool_choice: current_tool_choice,
        parallel_tool_calls: current_parallel_tool_calls,
        reasoning: current_reasoning,
        store: current_store,
        stream: current_stream,
        include: current_include,
        service_tier: current_service_tier,
        prompt_cache_key: current_prompt_cache_key,
        text: current_text,
        client_metadata: _,
        max_output_tokens: current_max_output_tokens,
    } = current;

    previous_model == current_model
        && previous_instructions == current_instructions
        && previous_tools == current_tools
        && previous_tool_choice == current_tool_choice
        && previous_parallel_tool_calls == current_parallel_tool_calls
        && previous_reasoning == current_reasoning
        && previous_store == current_store
        && previous_stream == current_stream
        && previous_include == current_include
        && previous_service_tier == current_service_tier
        && previous_prompt_cache_key == current_prompt_cache_key
        && previous_text == current_text
        && previous_max_output_tokens == current_max_output_tokens
}

impl WebsocketSession {
    fn set_connection_reused(&self, connection_reused: bool) {
        *self
            .connection_reused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = connection_reused;
    }

    fn connection_reused(&self) -> bool {
        *self
            .connection_reused
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

enum WebsocketStreamOutcome {
    Stream(ResponseStream),
    FallbackToHttp,
}

/// Result of opening a WebRTC Realtime call.
///
/// The SDP answer goes back to the client. The call id and auth headers stay on the server so the
/// ordinary Realtime WebSocket machinery can join the same in-progress call as a sideband
/// controller.
pub(crate) struct RealtimeWebrtcCallStart {
    pub(crate) sdp: String,
    pub(crate) call_id: String,
    pub(crate) sideband_headers: ApiHeaderMap,
}

/// Reuses the API-auth material that created the WebRTC call for the sideband WebSocket join.
///
/// API-key sessions send that API bearer. ChatGPT-auth sessions send their bearer plus account id;
/// transceiver is responsible for accepting that same call-create identity on the direct
/// `api.openai.com` sideband path.
fn sideband_websocket_auth_headers(api_auth: &dyn AuthProvider) -> ApiHeaderMap {
    let mut headers = ApiHeaderMap::new();
    api_auth.add_auth_headers(&mut headers);
    headers
}

impl ModelClient {
    #[allow(clippy::too_many_arguments)]
    /// Creates a new session-scoped `ModelClient`.
    ///
    /// All arguments are expected to be stable for the lifetime of a Forall session. Per-turn values
    /// are passed to [`ModelClientSession::stream`] (and other turn-scoped methods) explicitly.
    pub fn new(
        auth_manager: Option<Arc<AuthManager>>,
        thread_id: ThreadId,
        provider_info: ModelProviderInfo,
        session_source: SessionSource,
        model_verbosity: Option<VerbosityConfig>,
        enable_request_compression: bool,
        include_timing_metrics: bool,
        beta_features_header: Option<String>,
        item_ids_enabled: bool,
        attestation_provider: Option<Arc<dyn AttestationProvider>>,
    ) -> Self {
        let model_provider = create_model_provider(provider_info, auth_manager);
        let forall_api_key_env_enabled = model_provider
            .auth_manager()
            .as_ref()
            .is_some_and(|manager| manager.forall_api_key_env_enabled());
        let auth_env_telemetry =
            collect_auth_env_telemetry(model_provider.info(), forall_api_key_env_enabled);
        let include_attestation = model_provider.supports_attestation();
        Self {
            state: Arc::new(ModelClientState {
                thread_id,
                provider: model_provider,
                auth_env_telemetry,
                session_source,
                model_verbosity,
                enable_request_compression,
                include_timing_metrics,
                beta_features_header,
                item_ids_enabled,
                include_attestation,
                attestation_provider,
                disable_websockets: AtomicBool::new(false),
                cached_websocket_session: StdMutex::new(WebsocketSession::default()),
            }),
            prompt_cache_key_override: None,
        }
    }

    pub(crate) fn with_prompt_cache_key_override(
        mut self,
        prompt_cache_key_override: Option<String>,
    ) -> Self {
        self.prompt_cache_key_override = prompt_cache_key_override;
        self
    }

    fn prompt_cache_key(&self) -> String {
        self.prompt_cache_key_override
            .clone()
            .unwrap_or_else(|| self.state.thread_id.to_string())
    }

    /// Creates a fresh turn-scoped streaming session.
    ///
    /// This constructor does not perform network I/O itself; the session opens a websocket lazily
    /// when the first stream request is issued.
    pub fn new_session(&self) -> ModelClientSession {
        ModelClientSession {
            client: self.clone(),
            websocket_session: self.take_cached_websocket_session(),
            turn_state: Arc::new(OnceLock::new()),
        }
    }

    pub(crate) fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.state.provider.auth_manager()
    }

    fn take_cached_websocket_session(&self) -> WebsocketSession {
        let mut cached_websocket_session = self
            .state
            .cached_websocket_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *cached_websocket_session)
    }

    fn store_cached_websocket_session(&self, websocket_session: WebsocketSession) {
        *self
            .state
            .cached_websocket_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = websocket_session;
    }

    pub(crate) fn force_http_fallback(
        &self,
        session_telemetry: &SessionTelemetry,
        _model_info: &ModelInfo,
    ) -> bool {
        let websocket_enabled = self.responses_websocket_enabled();
        let activated =
            websocket_enabled && !self.state.disable_websockets.swap(true, Ordering::Relaxed);
        if activated {
            warn!("falling back to HTTP");
            session_telemetry.counter(
                "forall.transport.fallback_to_http",
                /*inc*/ 1,
                &[("from_wire_api", "responses_websocket")],
            );
        }

        self.store_cached_websocket_session(WebsocketSession::default());
        activated
    }

    /// Compacts the current conversation history using the Compact endpoint.
    ///
    /// This is a unary call (no streaming) that returns a new list of
    /// `ResponseItem`s representing the compacted transcript.
    ///
    /// The model selection and telemetry context are passed explicitly to keep `ModelClient`
    /// session-scoped.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn compact_conversation_history(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        turn_state: Option<Arc<OnceLock<String>>>,
        settings: CompactConversationRequestSettings,
        session_telemetry: &SessionTelemetry,
        compaction_trace: &CompactionTraceContext,
        responses_metadata: &ForallResponsesMetadata,
    ) -> Result<Vec<ResponseItem>> {
        if prompt.input.is_empty() {
            return Ok(Vec::new());
        }
        let client_setup = self.current_client_setup().await?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let request_telemetry = Self::build_request_telemetry(
            session_telemetry,
            AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                PendingUnauthorizedRetry::default(),
            ),
            RequestRouteTelemetry::for_endpoint(RESPONSES_COMPACT_ENDPOINT),
            self.state.auth_env_telemetry.clone(),
        );
        let request = self.build_responses_request(
            &client_setup.api_provider,
            prompt,
            model_info,
            settings.effort,
            settings.summary,
            settings.service_tier,
            responses_metadata,
        )?;
        let ResponsesApiRequest {
            model,
            instructions,
            mut input,
            tools,
            parallel_tool_calls,
            reasoning,
            service_tier,
            prompt_cache_key,
            text,
            ..
        } = request;
        self.prepare_response_items_for_request(&mut input, /*store*/ false);
        let payload = ApiCompactionInput {
            model: &model,
            input: &input,
            instructions: &instructions,
            tools,
            parallel_tool_calls,
            reasoning,
            service_tier: service_tier.as_deref(),
            prompt_cache_key: prompt_cache_key.as_deref(),
            text,
        };

        let mut extra_headers = ApiHeaderMap::new();
        if let Ok(header_value) = HeaderValue::from_str(&responses_metadata.installation_id) {
            extra_headers.insert(X_CODEX_INSTALLATION_ID_HEADER, header_value);
        }
        extra_headers.extend(build_responses_headers(
            self.state.beta_features_header.as_deref(),
            turn_state.as_ref(),
        ));
        extra_headers.extend(self.build_responses_compatibility_headers(responses_metadata));
        extra_headers.extend(build_session_headers(
            Some(responses_metadata.session_id.to_string()),
            Some(responses_metadata.thread_id.to_string()),
        ));
        if let Some(header_value) = self.generate_attestation_header_for().await {
            extra_headers.insert(X_OAI_ATTESTATION_HEADER, header_value);
        }
        add_responses_lite_header(&mut extra_headers, model_info.use_responses_lite);
        let compact_request_timeout = client_setup
            .api_provider
            .stream_idle_timeout
            .saturating_mul(COMPACT_REQUEST_TIMEOUT_IDLE_MULTIPLIER);
        let client =
            ApiCompactClient::new(transport, client_setup.api_provider, client_setup.api_auth)
                .with_telemetry(Some(request_telemetry));
        let trace_attempt = compaction_trace.start_attempt(&payload);
        let result = client
            .compact_input(
                &payload,
                extra_headers,
                compact_request_timeout,
                turn_state.as_deref(),
            )
            .await
            .map_err(|error| self.state.provider.map_api_error(error));
        trace_attempt.record_result(result.as_deref());
        result
    }

    pub(crate) async fn create_realtime_call_with_headers(
        &self,
        sdp: String,
        session_config: ApiRealtimeSessionConfig,
        mut extra_headers: ApiHeaderMap,
        api_provider_override: Option<ApiProvider>,
    ) -> Result<RealtimeWebrtcCallStart> {
        // Create the media call over HTTP first, then retain matching auth so realtime can attach
        // the server-side control WebSocket to the call id from that HTTP response.
        let client_setup = self.current_client_setup().await?;
        if let Some(header_value) = self.generate_attestation_header_for().await {
            extra_headers.insert(X_OAI_ATTESTATION_HEADER, header_value);
        }
        let mut sideband_headers = extra_headers.clone();
        sideband_headers.extend(sideband_websocket_auth_headers(
            client_setup.api_auth.as_ref(),
        ));
        let transport = ReqwestTransport::new(build_reqwest_client());
        let api_provider = api_provider_override.unwrap_or(client_setup.api_provider);
        let response = ApiRealtimeCallClient::new(transport, api_provider, client_setup.api_auth)
            .create_with_session_and_headers(sdp, session_config, extra_headers)
            .await
            .map_err(|error| self.state.provider.map_api_error(error))?;
        Ok(RealtimeWebrtcCallStart {
            sdp: response.sdp,
            call_id: response.call_id,
            sideband_headers,
        })
    }

    /// Builds memory summaries for each provided normalized raw memory.
    ///
    /// This is a unary call (no streaming) to `/v1/memories/trace_summarize`.
    ///
    /// The model selection, reasoning effort, and telemetry context are passed explicitly to keep
    /// `ModelClient` session-scoped.
    pub async fn summarize_memories(
        &self,
        raw_memories: Vec<ApiRawMemory>,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
        session_telemetry: &SessionTelemetry,
    ) -> Result<Vec<ApiMemorySummarizeOutput>> {
        if raw_memories.is_empty() {
            return Ok(Vec::new());
        }

        let client_setup = self.current_client_setup().await?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let request_telemetry = Self::build_request_telemetry(
            session_telemetry,
            AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                PendingUnauthorizedRetry::default(),
            ),
            RequestRouteTelemetry::for_endpoint(MEMORIES_SUMMARIZE_ENDPOINT),
            self.state.auth_env_telemetry.clone(),
        );
        let client =
            ApiMemoriesClient::new(transport, client_setup.api_provider, client_setup.api_auth)
                .with_telemetry(Some(request_telemetry));

        let payload = ApiMemorySummarizeInput {
            model: model_info.slug.clone(),
            raw_memories,
            reasoning: effort.map(|effort| Reasoning {
                effort: Some(effort),
                summary: None,
                context: None,
            }),
        };

        client
            .summarize_input(&payload, self.build_subagent_headers())
            .await
            .map_err(|error| self.state.provider.map_api_error(error))
    }

    fn build_subagent_headers(&self) -> ApiHeaderMap {
        let mut extra_headers = ApiHeaderMap::new();
        if let Some(subagent) = subagent_header_value(&self.state.session_source)
            && let Ok(val) = HeaderValue::from_str(&subagent)
        {
            extra_headers.insert(X_OPENAI_SUBAGENT_HEADER, val);
        }
        if matches!(
            self.state.session_source,
            SessionSource::Internal(InternalSessionSource::MemoryConsolidation)
        ) {
            extra_headers.insert(
                X_OPENAI_MEMGEN_REQUEST_HEADER,
                HeaderValue::from_static("true"),
            );
        }
        extra_headers
    }

    fn build_responses_compatibility_headers(
        &self,
        responses_metadata: &ForallResponsesMetadata,
    ) -> ApiHeaderMap {
        let mut extra_headers = responses_metadata.compatibility_headers();
        if matches!(
            self.state.session_source,
            SessionSource::Internal(InternalSessionSource::MemoryConsolidation)
        ) {
            extra_headers.insert(
                X_OPENAI_MEMGEN_REQUEST_HEADER,
                HeaderValue::from_static("true"),
            );
        }
        extra_headers
    }

    fn build_ws_client_metadata(
        &self,
        responses_metadata: &ForallResponsesMetadata,
        use_responses_lite: bool,
    ) -> HashMap<String, String> {
        let mut client_metadata = responses_metadata.client_metadata();
        if use_responses_lite {
            client_metadata.insert(
                WS_REQUEST_HEADER_RESPONSES_LITE_CLIENT_METADATA_KEY.to_string(),
                "true".to_string(),
            );
        }
        client_metadata
    }

    async fn generate_attestation_header_for(&self) -> Option<HeaderValue> {
        if !self.state.include_attestation {
            return None;
        }

        self.state
            .attestation_provider
            .as_ref()?
            .header_for_request(AttestationContext {
                thread_id: self.state.thread_id,
            })
            .await
    }

    /// Builds request telemetry for unary API calls (e.g., Compact endpoint).
    fn build_request_telemetry(
        session_telemetry: &SessionTelemetry,
        auth_context: AuthRequestTelemetryContext,
        request_route_telemetry: RequestRouteTelemetry,
        auth_env_telemetry: AuthEnvTelemetry,
    ) -> Arc<dyn RequestTelemetry> {
        let telemetry = Arc::new(ApiTelemetry::new(
            session_telemetry.clone(),
            auth_context,
            request_route_telemetry,
            auth_env_telemetry,
        ));
        let request_telemetry: Arc<dyn RequestTelemetry> = telemetry;
        request_telemetry
    }

    fn resolve_reasoning_effort(
        effort: Option<ReasoningEffortConfig>,
        model_info: &ModelInfo,
    ) -> Option<ReasoningEffortConfig> {
        match effort {
            None | Some(ReasoningEffortConfig::None) => model_info.default_reasoning_level.clone(),
            Some(effort) => Some(effort),
        }
    }

    fn build_reasoning(
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
    ) -> Option<Reasoning> {
        if model_info.supports_reasoning_summaries {
            Some(Reasoning {
                effort: Self::resolve_reasoning_effort(effort, model_info),
                summary: if summary == ReasoningSummaryConfig::None {
                    None
                } else {
                    Some(summary)
                },
                // When Responses Lite is disabled, omit context so Responses uses the default,
                // which is currently `current_turn`.
                context: model_info
                    .use_responses_lite
                    .then_some(ReasoningContext::AllTurns),
            })
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_responses_request(
        &self,
        provider: &forall_api::Provider,
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &ForallResponsesMetadata,
    ) -> Result<ResponsesApiRequest> {
        let instructions = &prompt.base_instructions.text;
        let mut input = prompt.get_formatted_input_for_request(model_info.use_responses_lite);
        if !self.state.provider.info().is_openai() {
            input
                .iter_mut()
                .for_each(ResponseItem::clear_internal_chat_message_metadata_passthrough);
        }
        let tools = create_tools_json_for_responses_api(&prompt.tools)?;
        let reasoning = Self::build_reasoning(model_info, effort, summary);
        let include = if reasoning.is_some() {
            vec!["reasoning.encrypted_content".to_string()]
        } else {
            Vec::new()
        };
        let verbosity = if model_info.support_verbosity {
            self.state.model_verbosity.or(model_info.default_verbosity)
        } else {
            if self.state.model_verbosity.is_some() {
                warn!(
                    "model_verbosity is set but ignored as the model does not support verbosity: {}",
                    model_info.slug
                );
            }
            None
        };
        let text = create_text_param_for_request(
            verbosity,
            &prompt.output_schema,
            prompt.output_schema_strict,
        );
        let prompt_cache_key = Some(self.prompt_cache_key());
        let service_tier = model_info.service_tier_for_request(service_tier);
        let max_output_tokens = if self.state.provider.info().is_openai() {
            None
        } else {
            Some(8_192)
        };
        let request = ResponsesApiRequest {
            model: model_info.slug.clone(),
            instructions: instructions.clone(),
            input,
            tools,
            tool_choice: "auto".to_string(),
            parallel_tool_calls: prompt.parallel_tool_calls && !model_info.use_responses_lite,
            reasoning,
            store: provider.is_azure_responses_endpoint(),
            stream: true,
            include,
            service_tier,
            prompt_cache_key,
            text,
            client_metadata: Some(responses_metadata.client_metadata()),
            max_output_tokens,
        };
        Ok(request)
    }

    /// Builds the request body for the Anthropic Messages API.
    ///
    /// Responses-only parameters are intentionally NOT sent on this wire
    /// protocol:
    /// - `prompt_cache_key`: Anthropic caching is positional — explicit
    ///   `cache_control: {"type": "ephemeral"}` breakpoints placed here (on
    ///   the system block, the last tool definition, and the previous user
    ///   turn) replace the opaque server-side cache key.
    /// - `store` / `include`: server-side response storage and
    ///   `include: ["reasoning.encrypted_content"]` are Responses concepts;
    ///   thinking blocks round-trip through the envelope synthesized into
    ///   `ResponseItem::Reasoning::encrypted_content` instead.
    /// - `service_tier`, `verbosity`, and the `text` controls (output
    ///   schemas): no Messages API equivalents exist.
    fn build_messages_request(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
    ) -> Result<MessagesApiRequest> {
        // Cache breakpoint 1 (of at most 4): the system prompt is the most
        // stable segment of the prefix.
        let system = (!prompt.base_instructions.text.is_empty()).then(|| {
            vec![MessagesSystemBlock::Text {
                text: prompt.base_instructions.text.clone(),
                cache_control: Some(CacheControl::Ephemeral),
            }]
        });

        let mut tools: Vec<MessagesToolSpec> = create_tools_json_for_messages_api(&prompt.tools)?
            .into_iter()
            .map(|tool| MessagesToolSpec {
                name: tool.name,
                description: tool.description,
                input_schema: tool.input_schema,
                cache_control: None,
            })
            .collect();
        // Cache breakpoint 2: a breakpoint on the last tool definition caches
        // the entire `tools` array, which rarely changes within a session.
        if let Some(last_tool) = tools.last_mut() {
            last_tool.cache_control = Some(CacheControl::Ephemeral);
        }
        // `tool_choice` is only valid when tools are present. Parallel tool
        // use is opt-out on this API (inverted polarity vs. the Responses
        // `parallel_tool_calls` flag), so it is disabled only when the prompt
        // does not permit parallel calls.
        let tool_choice = (!tools.is_empty()).then(|| MessagesToolChoice::Auto {
            disable_parallel_tool_use: (!prompt.parallel_tool_calls).then_some(true),
        });

        let mut messages = build_messages_history(&prompt.input);
        // Cache breakpoint 3.
        apply_history_cache_breakpoint(&mut messages);

        let thinking = if model_info.supports_reasoning_summaries {
            Self::resolve_reasoning_effort(effort, model_info)
                .as_ref()
                .and_then(thinking_budget_tokens)
                .map(|budget_tokens| ThinkingConfig::Enabled { budget_tokens })
        } else {
            None
        };

        Ok(MessagesApiRequest {
            model: model_info.slug.clone(),
            system,
            messages,
            tools,
            tool_choice,
            max_tokens: MESSAGES_DEFAULT_MAX_OUTPUT_TOKENS,
            thinking,
            stream: true,
            metadata: None,
        })
    }

    /// Builds the request body for the native Gemini `generateContent` API.
    ///
    /// Responses-only parameters are intentionally NOT sent on this wire
    /// protocol:
    /// - `prompt_cache_key`: Gemini caching is either implicit (automatic, with
    ///   no client control) or an explicitly created `cachedContents` resource
    ///   with its own lifecycle. Neither is an opaque per-request key, so there
    ///   is nothing to map it onto — and unlike the Messages adapter, there are
    ///   no positional breakpoints to place either.
    /// - `store` / `include`: server-side response storage and
    ///   `include: ["reasoning.encrypted_content"]` are Responses concepts;
    ///   thought parts round-trip through the envelope synthesized into
    ///   `ResponseItem::Reasoning::encrypted_content` instead.
    /// - `service_tier`, `verbosity`, and the `text` controls (output schemas):
    ///   no equivalents on this endpoint.
    /// - `parallel_tool_calls`: Gemini has no toggle for it. The model may
    ///   always return several `functionCall` parts in one response, so
    ///   `prompt.parallel_tool_calls` cannot be enforced on the wire; core's
    ///   own per-tool gating still applies when the calls come back.
    fn build_gemini_request(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffortConfig>,
    ) -> Result<GeminiRequest> {
        let system_instruction =
            (!prompt.base_instructions.text.is_empty()).then(|| GeminiSystemInstruction {
                parts: vec![GeminiPart::Text {
                    text: prompt.base_instructions.text.clone(),
                }],
            });

        let function_declarations: Vec<GeminiFunctionDeclaration> =
            create_tools_json_for_gemini_api(&prompt.tools)?
                .into_iter()
                .map(|tool| GeminiFunctionDeclaration {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.parameters,
                })
                .collect();
        // Gemini groups every function declaration under a single `tools`
        // entry; an empty entry is invalid, so the array stays empty instead.
        let has_tools = !function_declarations.is_empty();
        let tools = if has_tools {
            vec![GeminiTool {
                function_declarations,
            }]
        } else {
            Vec::new()
        };
        // `toolConfig` is only meaningful when tools are present.
        let tool_config = has_tools.then_some(GeminiToolConfig {
            function_calling_config: GeminiFunctionCallingConfig {
                mode: GeminiFunctionCallingMode::Auto,
            },
        });

        let thinking_config = if model_info.supports_reasoning_summaries {
            Self::resolve_reasoning_effort(effort, model_info)
                .as_ref()
                .and_then(gemini_thinking_budget)
                .map(|thinking_budget| GeminiThinkingConfig {
                    thinking_budget: Some(thinking_budget),
                    // Without this the budget is still spent but no thought
                    // parts are streamed, so there would be nothing to show or
                    // to round-trip.
                    include_thoughts: Some(true),
                })
        } else {
            None
        };

        Ok(GeminiRequest {
            contents: build_gemini_contents(&prompt.input),
            system_instruction,
            tools,
            tool_config,
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(GEMINI_DEFAULT_MAX_OUTPUT_TOKENS),
                // No core setting maps onto Gemini's sampling temperature, so
                // the model default is left in place rather than guessed at.
                temperature: None,
                thinking_config,
            }),
        })
    }

    fn prepare_response_items_for_request(&self, input: &mut [ResponseItem], store: bool) {
        if self.state.item_ids_enabled || store {
            return;
        }

        for item in input {
            item.set_id(/*new_id*/ None);
        }
    }

    /// Returns whether the Responses-over-WebSocket transport is active for this session.
    ///
    /// WebSocket use is controlled by provider capability and session-scoped fallback state.
    pub fn responses_websocket_enabled(&self) -> bool {
        if !self.state.provider.info().supports_websockets
            || self.state.disable_websockets.load(Ordering::Relaxed)
        {
            return false;
        }

        true
    }

    /// Returns auth + provider configuration resolved from the current session auth state.
    ///
    /// This centralizes setup used by both prewarm and normal request paths so they stay in
    /// lockstep when auth/provider resolution changes.
    async fn current_client_setup(&self) -> Result<CurrentClientSetup> {
        let auth = self.state.provider.auth().await;
        let api_provider = self.state.provider.api_provider().await?;
        let api_auth = self.state.provider.api_auth().await?;
        Ok(CurrentClientSetup {
            auth,
            api_provider,
            api_auth,
        })
    }

    /// Opens a websocket connection using the same header and telemetry wiring as normal turns.
    ///
    /// Both startup prewarm and in-turn `needs_new` reconnects call this path so handshake
    /// behavior remains consistent across both flows.
    #[allow(clippy::too_many_arguments)]
    async fn connect_websocket(
        &self,
        session_telemetry: &SessionTelemetry,
        api_provider: forall_api::Provider,
        api_auth: SharedAuthProvider,
        responses_metadata: &ForallResponsesMetadata,
        auth_context: AuthRequestTelemetryContext,
        request_route_telemetry: RequestRouteTelemetry,
    ) -> std::result::Result<ApiWebSocketConnection, ApiError> {
        let headers = self.build_websocket_headers(responses_metadata).await;
        let websocket_telemetry = ModelClientSession::build_websocket_telemetry(
            session_telemetry,
            auth_context,
            request_route_telemetry,
            self.state.auth_env_telemetry.clone(),
        );
        let websocket_connect_timeout = self.state.provider.info().websocket_connect_timeout();
        let start = Instant::now();
        let result = match tokio::time::timeout(
            websocket_connect_timeout,
            ApiWebSocketResponsesClient::new(api_provider, api_auth).connect(
                headers,
                forall_login::default_client::default_headers(),
                /*turn_state*/ None,
                Some(websocket_telemetry),
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ApiError::Transport(TransportError::Timeout)),
        };
        let error_message = result.as_ref().err().map(telemetry_api_error_message);
        let response_debug = result
            .as_ref()
            .err()
            .map(extract_response_debug_context_from_api_error)
            .unwrap_or_default();
        let status = result.as_ref().err().and_then(api_error_http_status);
        session_telemetry.record_websocket_connect(
            start.elapsed(),
            status,
            error_message.as_deref(),
            auth_context.auth_header_attached,
            auth_context.auth_header_name,
            auth_context.retry_after_unauthorized,
            auth_context.recovery_mode,
            auth_context.recovery_phase,
            request_route_telemetry.endpoint,
            /*connection_reused*/ false,
            response_debug.request_id.as_deref(),
            response_debug.cf_ray.as_deref(),
            response_debug.auth_error.as_deref(),
            response_debug.auth_error_code.as_deref(),
        );
        emit_feedback_request_tags_with_auth_env(
            &FeedbackRequestTags {
                endpoint: request_route_telemetry.endpoint,
                auth_header_attached: auth_context.auth_header_attached,
                auth_header_name: auth_context.auth_header_name,
                auth_mode: auth_context.auth_mode,
                auth_retry_after_unauthorized: Some(auth_context.retry_after_unauthorized),
                auth_recovery_mode: auth_context.recovery_mode,
                auth_recovery_phase: auth_context.recovery_phase,
                auth_connection_reused: Some(false),
                auth_request_id: response_debug.request_id.as_deref(),
                auth_cf_ray: response_debug.cf_ray.as_deref(),
                auth_error: response_debug.auth_error.as_deref(),
                auth_error_code: response_debug.auth_error_code.as_deref(),
                auth_recovery_followup_success: auth_context
                    .retry_after_unauthorized
                    .then_some(result.is_ok()),
                auth_recovery_followup_status: auth_context
                    .retry_after_unauthorized
                    .then_some(status)
                    .flatten(),
            },
            &self.state.auth_env_telemetry,
        );
        result
    }

    /// Builds websocket handshake headers for both prewarm and turn-time reconnect.
    async fn build_websocket_headers(
        &self,
        responses_metadata: &ForallResponsesMetadata,
    ) -> ApiHeaderMap {
        let mut headers = build_responses_headers(
            self.state.beta_features_header.as_deref(),
            /*turn_state*/ None,
        );
        if let Ok(header_value) = HeaderValue::from_str(&responses_metadata.thread_id) {
            headers.insert("x-client-request-id", header_value);
        }
        headers.extend(build_session_headers(
            Some(responses_metadata.session_id.to_string()),
            Some(responses_metadata.thread_id.to_string()),
        ));
        headers.extend(self.build_responses_compatibility_headers(responses_metadata));
        if let Some(header_value) = self.generate_attestation_header_for().await {
            headers.insert(X_OAI_ATTESTATION_HEADER, header_value);
        }
        headers.insert(
            OPENAI_BETA_HEADER,
            HeaderValue::from_static(RESPONSES_WEBSOCKETS_V2_BETA_HEADER_VALUE),
        );
        if self.state.include_timing_metrics {
            headers.insert(
                X_RESPONSESAPI_INCLUDE_TIMING_METRICS_HEADER,
                HeaderValue::from_static("true"),
            );
        }
        headers
    }
}

impl Drop for ModelClientSession {
    fn drop(&mut self) {
        let websocket_session = std::mem::take(&mut self.websocket_session);
        self.client
            .store_cached_websocket_session(websocket_session);
    }
}

impl ModelClientSession {
    pub(crate) fn turn_state(&self) -> Arc<OnceLock<String>> {
        Arc::clone(&self.turn_state)
    }

    fn reset_websocket_session(&mut self) {
        self.websocket_session.connection = None;
        self.websocket_session.last_request = None;
        self.websocket_session.last_response_rx = None;
        self.websocket_session.last_response_from_untraced_warmup = false;
        self.websocket_session
            .set_connection_reused(/*connection_reused*/ false);
    }

    #[allow(clippy::too_many_arguments)]
    /// Builds shared Responses API transport options and request-body options.
    ///
    /// Keeping option construction in one place ensures request-scoped headers are consistent
    /// regardless of transport choice.
    async fn build_responses_options(
        &self,
        responses_metadata: &ForallResponsesMetadata,
        compression: Compression,
        use_responses_lite: bool,
    ) -> ApiResponsesOptions {
        ApiResponsesOptions {
            session_id: Some(responses_metadata.session_id.to_string()),
            thread_id: Some(responses_metadata.thread_id.to_string()),
            session_source: Some(self.client.state.session_source.clone()),
            extra_headers: {
                let mut headers = build_responses_headers(
                    self.client.state.beta_features_header.as_deref(),
                    Some(&self.turn_state),
                );
                headers.extend(
                    self.client
                        .build_responses_compatibility_headers(responses_metadata),
                );
                if let Some(header_value) = self.client.generate_attestation_header_for().await {
                    headers.insert(X_OAI_ATTESTATION_HEADER, header_value);
                }
                add_responses_lite_header(&mut headers, use_responses_lite);
                headers
            },
            compression,
            turn_state: Some(Arc::clone(&self.turn_state)),
        }
    }

    fn get_incremental_items(
        &self,
        request: &ResponsesApiRequest,
        last_response: Option<&LastResponse>,
        allow_empty_delta: bool,
    ) -> Option<Vec<ResponseItem>> {
        // Checks whether the current request is an incremental extension of the previous request.
        // We only reuse an incremental input delta when non-input request fields are unchanged and
        // `input` is a strict
        // extension of the previous known input. Server-returned output items are treated as part
        // of the baseline so we do not resend them.
        let previous_request = self.websocket_session.last_request.as_ref()?;
        if !responses_request_properties_match(previous_request, request) {
            trace!("incremental request failed, websocket reuse properties didn't match");
            return None;
        }

        let Some(after_previous_input) = request
            .input
            .strip_prefix(previous_request.input.as_slice())
        else {
            trace!("incremental request failed, items didn't match");
            return None;
        };
        let mut response_items =
            last_response.map_or_else(Vec::new, |response| response.items_added.clone());
        if !self.client.state.provider.info().is_openai() {
            response_items
                .iter_mut()
                .for_each(ResponseItem::clear_internal_chat_message_metadata_passthrough);
        }
        let Some(incremental_items) = after_previous_input.strip_prefix(response_items.as_slice())
        else {
            trace!("incremental request failed, items didn't match");
            return None;
        };
        if !allow_empty_delta && incremental_items.is_empty() {
            return None;
        }
        Some(incremental_items.to_vec())
    }

    fn get_last_response(&mut self) -> Option<LastResponse> {
        self.websocket_session
            .last_response_rx
            .take()
            .and_then(|mut receiver| match receiver.try_recv() {
                Ok(last_response) => Some(last_response),
                Err(TryRecvError::Closed) | Err(TryRecvError::Empty) => None,
            })
    }

    fn prepare_websocket_request(
        &mut self,
        payload: ResponseCreateWsRequest,
        request: &ResponsesApiRequest,
    ) -> (ResponsesWsRequest, bool) {
        let Some(last_response) = self.get_last_response() else {
            return (ResponsesWsRequest::ResponseCreate(payload), false);
        };
        let previous_response_id_from_untraced_warmup =
            self.websocket_session.last_response_from_untraced_warmup;
        let Some(incremental_items) = self.get_incremental_items(
            request,
            Some(&last_response),
            /*allow_empty_delta*/ true,
        ) else {
            return (ResponsesWsRequest::ResponseCreate(payload), false);
        };

        if last_response.response_id.is_empty() {
            trace!("incremental request failed, no previous response id");
            return (ResponsesWsRequest::ResponseCreate(payload), false);
        }

        (
            ResponsesWsRequest::ResponseCreate(ResponseCreateWsRequest {
                previous_response_id: Some(last_response.response_id),
                input: incremental_items,
                ..payload
            }),
            previous_response_id_from_untraced_warmup,
        )
    }

    /// Opportunistically preconnects a websocket for this turn-scoped client session.
    ///
    /// This performs only connection setup; it never sends prompt payloads.
    pub async fn preconnect_websocket(
        &mut self,
        session_telemetry: &SessionTelemetry,
        responses_metadata: &ForallResponsesMetadata,
    ) -> std::result::Result<(), ApiError> {
        if !self.client.responses_websocket_enabled() {
            return Ok(());
        }
        if self.websocket_session.connection.is_some() {
            return Ok(());
        }

        let client_setup = self.client.current_client_setup().await.map_err(|err| {
            ApiError::Stream(format!(
                "failed to build websocket prewarm client setup: {err}"
            ))
        })?;
        let auth_context = AuthRequestTelemetryContext::new(
            client_setup.auth.as_ref().map(ForallAuth::auth_mode),
            client_setup.api_auth.as_ref(),
            PendingUnauthorizedRetry::default(),
        );
        let connection = self
            .client
            .connect_websocket(
                session_telemetry,
                client_setup.api_provider,
                client_setup.api_auth,
                responses_metadata,
                auth_context,
                RequestRouteTelemetry::for_endpoint(RESPONSES_ENDPOINT),
            )
            .await?;
        self.websocket_session.connection = Some(connection);
        self.websocket_session
            .set_connection_reused(/*connection_reused*/ false);
        Ok(())
    }
    /// Returns a websocket connection for this turn.
    #[instrument(
        name = "model_client.websocket_connection",
        level = "info",
        skip_all,
        fields(
            provider = %self.client.state.provider.info().name,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "responses_websocket",
            api.path = "responses",
            turn.has_metadata_header = params.responses_metadata.has_turn_metadata()
        )
    )]
    async fn websocket_connection(
        &mut self,
        params: WebsocketConnectParams<'_>,
    ) -> std::result::Result<&ApiWebSocketConnection, ApiError> {
        let WebsocketConnectParams {
            session_telemetry,
            api_provider,
            api_auth,
            responses_metadata,
            auth_context,
            request_route_telemetry,
        } = params;
        let needs_new = match self.websocket_session.connection.as_ref() {
            Some(conn) => conn.is_closed().await,
            None => true,
        };

        if needs_new {
            self.websocket_session.last_request = None;
            self.websocket_session.last_response_rx = None;
            self.websocket_session.last_response_from_untraced_warmup = false;
            let new_conn = match self
                .client
                .connect_websocket(
                    session_telemetry,
                    api_provider,
                    api_auth,
                    responses_metadata,
                    auth_context,
                    request_route_telemetry,
                )
                .await
            {
                Ok(new_conn) => new_conn,
                Err(err) => {
                    if matches!(err, ApiError::Transport(TransportError::Timeout)) {
                        self.reset_websocket_session();
                    }
                    return Err(err);
                }
            };
            self.websocket_session.connection = Some(new_conn);
            self.websocket_session
                .set_connection_reused(/*connection_reused*/ false);
        } else {
            self.websocket_session
                .set_connection_reused(/*connection_reused*/ true);
        }

        self.websocket_session
            .connection
            .as_ref()
            .ok_or(ApiError::Stream(
                "websocket connection is unavailable".to_string(),
            ))
    }

    fn responses_request_compression(&self, auth: Option<&ForallAuth>) -> Compression {
        if self.client.state.enable_request_compression
            && auth.is_some_and(ForallAuth::uses_forall_backend)
            && self.client.state.provider.info().is_openai()
        {
            Compression::Zstd
        } else {
            Compression::None
        }
    }

    /// Streams a turn via the OpenAI Responses API.
    ///
    /// Handles reasoning summaries, verbosity, and the `text` controls used for output schemas.
    #[allow(clippy::too_many_arguments)]
    #[instrument(
        name = "model_client.stream_responses_api",
        level = "info",
        skip_all,
        fields(
            model = %model_info.slug,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "responses_http",
            http.method = "POST",
            api.path = "responses",
            turn.has_metadata_header = responses_metadata.has_turn_metadata()
        )
    )]
    async fn stream_responses_api(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &ForallResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let auth_manager = self.client.state.provider.auth_manager();
        let mut auth_recovery = auth_manager
            .as_ref()
            .map(AuthManager::unauthorized_recovery);
        let mut pending_retry = PendingUnauthorizedRetry::default();
        loop {
            let client_setup = self.client.current_client_setup().await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let request_auth_context = AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                pending_retry,
            );
            let (request_telemetry, sse_telemetry) = Self::build_streaming_telemetry(
                session_telemetry,
                request_auth_context,
                RequestRouteTelemetry::for_endpoint(RESPONSES_ENDPOINT),
                self.client.state.auth_env_telemetry.clone(),
            );
            let compression = self.responses_request_compression(client_setup.auth.as_ref());
            let mut options = self
                .build_responses_options(
                    responses_metadata,
                    compression,
                    model_info.use_responses_lite,
                )
                .await;

            let mut request = self.client.build_responses_request(
                &client_setup.api_provider,
                prompt,
                model_info,
                effort.clone(),
                summary,
                service_tier.clone(),
                responses_metadata,
            )?;
            let store = request.store;
            self.client
                .prepare_response_items_for_request(&mut request.input, store);
            let request_session_telemetry =
                session_telemetry_for_request(session_telemetry, &request);
            let inference_trace_attempt = inference_trace.start_attempt();
            inference_trace_attempt.add_request_headers(&mut options.extra_headers);
            inference_trace_attempt.record_started(&request);
            let client = ApiResponsesClient::new(
                transport,
                client_setup.api_provider,
                client_setup.api_auth,
            )
            .with_telemetry(Some(request_telemetry), Some(sse_telemetry));
            let stream_result = client.stream_request(request, options).await;

            match stream_result {
                Ok(stream) => {
                    let (stream, _) = map_response_stream(
                        stream,
                        request_session_telemetry,
                        inference_trace_attempt,
                        Arc::clone(&self.client.state.provider),
                    );
                    return Ok(stream);
                }
                Err(ApiError::Transport(
                    unauthorized_transport @ TransportError::Http { status, .. },
                )) if status == StatusCode::UNAUTHORIZED => {
                    let response_debug_context =
                        extract_response_debug_context(&unauthorized_transport);
                    inference_trace_attempt.record_failed(
                        &unauthorized_transport,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    pending_retry = PendingUnauthorizedRetry::from_recovery(
                        handle_unauthorized(
                            unauthorized_transport,
                            &mut auth_recovery,
                            session_telemetry,
                            &self.client.state.provider,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(err) => {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let err = self.client.state.provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &err,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    return Err(err);
                }
            }
        }
    }

    /// Streams a turn via the native Gemini `generateContent` API.
    ///
    /// This mirrors [`Self::stream_messages_api`]: the same 401-recovery loop,
    /// the same telemetry wiring (routed to [`GEMINI_ENDPOINT`]), and the same
    /// [`map_response_stream`] adapter — the Gemini SSE layer already emits
    /// `forall_api::ResponseEvent`s, so everything downstream of the transport
    /// is shared with the Responses path.
    #[instrument(
        name = "model_client.stream_gemini_api",
        level = "info",
        skip_all,
        fields(
            model = %model_info.slug,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "gemini_http",
            http.method = "POST",
            api.path = "models:streamGenerateContent",
            turn.has_metadata_header = responses_metadata.has_turn_metadata()
        )
    )]
    async fn stream_gemini_api(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        responses_metadata: &ForallResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let auth_manager = self.client.state.provider.auth_manager();
        let mut auth_recovery = auth_manager
            .as_ref()
            .map(AuthManager::unauthorized_recovery);
        let mut pending_retry = PendingUnauthorizedRetry::default();
        loop {
            let client_setup = self.client.current_client_setup().await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let request_auth_context = AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                pending_retry,
            );
            let (request_telemetry, sse_telemetry) = Self::build_streaming_telemetry(
                session_telemetry,
                request_auth_context,
                RequestRouteTelemetry::for_endpoint(GEMINI_ENDPOINT),
                self.client.state.auth_env_telemetry.clone(),
            );
            let mut options = ApiGeminiOptions {
                extra_headers: ApiHeaderMap::new(),
                // Request compression is a Forall-backend affordance; Gemini
                // gets plain JSON.
                compression: Compression::None,
                // Gemini has no sticky-routing turn state.
                turn_state: None,
                // Freeform tools are advertised as single-property object
                // tools; the SSE adapter needs their names to unwrap the
                // resulting calls back into `CustomToolCall`.
                freeform_tool_names: forall_tools::freeform_tool_names(&prompt.tools),
            };

            let request = self
                .client
                .build_gemini_request(prompt, model_info, effort.clone())?;
            let resolved_effort = model_info
                .supports_reasoning_summaries
                .then(|| ModelClient::resolve_reasoning_effort(effort.clone(), model_info))
                .flatten();
            let request_session_telemetry = session_telemetry
                .clone()
                .with_inference_request(/*service_tier*/ None, resolved_effort.as_ref());
            let inference_trace_attempt = inference_trace.start_attempt();
            inference_trace_attempt.add_request_headers(&mut options.extra_headers);
            inference_trace_attempt.record_started(&request);
            let client =
                ApiGeminiClient::new(transport, client_setup.api_provider, client_setup.api_auth)
                    .with_telemetry(Some(request_telemetry), Some(sse_telemetry));
            let stream_result = client
                .stream_request(&model_info.slug, request, options)
                .await;

            match stream_result {
                Ok(stream) => {
                    let (stream, _) = map_response_stream(
                        stream,
                        request_session_telemetry,
                        inference_trace_attempt,
                        Arc::clone(&self.client.state.provider),
                    );
                    return Ok(stream);
                }
                Err(ApiError::Transport(
                    unauthorized_transport @ TransportError::Http { status, .. },
                )) if status == StatusCode::UNAUTHORIZED => {
                    let response_debug_context =
                        extract_response_debug_context(&unauthorized_transport);
                    inference_trace_attempt.record_failed(
                        &unauthorized_transport,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    pending_retry = PendingUnauthorizedRetry::from_recovery(
                        handle_unauthorized(
                            unauthorized_transport,
                            &mut auth_recovery,
                            session_telemetry,
                            &self.client.state.provider,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(err) => {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let err = self.client.state.provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &err,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    return Err(err);
                }
            }
        }
    }

    /// Streams a turn via the Anthropic Messages API.
    ///
    /// This mirrors [`Self::stream_responses_api`]: the same 401-recovery
    /// loop, the same telemetry wiring (routed to the `/messages` endpoint),
    /// and the same [`map_response_stream`] adapter — the Messages SSE layer
    /// already emits `forall_api::ResponseEvent`s, so everything downstream
    /// of the transport is shared with the Responses path.
    #[instrument(
        name = "model_client.stream_messages_api",
        level = "info",
        skip_all,
        fields(
            model = %model_info.slug,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "messages_http",
            http.method = "POST",
            api.path = "messages",
            turn.has_metadata_header = responses_metadata.has_turn_metadata()
        )
    )]
    async fn stream_messages_api(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        responses_metadata: &ForallResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let auth_manager = self.client.state.provider.auth_manager();
        let mut auth_recovery = auth_manager
            .as_ref()
            .map(AuthManager::unauthorized_recovery);
        let mut pending_retry = PendingUnauthorizedRetry::default();
        loop {
            let client_setup = self.client.current_client_setup().await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let request_auth_context = AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                pending_retry,
            );
            let (request_telemetry, sse_telemetry) = Self::build_streaming_telemetry(
                session_telemetry,
                request_auth_context,
                RequestRouteTelemetry::for_endpoint(MESSAGES_ENDPOINT),
                self.client.state.auth_env_telemetry.clone(),
            );
            let mut options = ApiMessagesOptions {
                session_id: Some(responses_metadata.session_id.to_string()),
                thread_id: Some(responses_metadata.thread_id.to_string()),
                session_source: Some(self.client.state.session_source.clone()),
                extra_headers: ApiHeaderMap::new(),
                // Request compression is a Forall-backend affordance; the
                // Messages API gets plain JSON.
                compression: Compression::None,
                // The Messages API has no sticky-routing turn state.
                turn_state: None,
                // Freeform tools are advertised as single-property object
                // tools; the SSE adapter needs their names to unwrap the
                // resulting calls back into `CustomToolCall`.
                freeform_tool_names: forall_tools::freeform_tool_names(&prompt.tools),
            };

            let request = self
                .client
                .build_messages_request(prompt, model_info, effort.clone())?;
            let resolved_effort = model_info
                .supports_reasoning_summaries
                .then(|| ModelClient::resolve_reasoning_effort(effort.clone(), model_info))
                .flatten();
            let request_session_telemetry = session_telemetry
                .clone()
                .with_inference_request(/*service_tier*/ None, resolved_effort.as_ref());
            let inference_trace_attempt = inference_trace.start_attempt();
            inference_trace_attempt.add_request_headers(&mut options.extra_headers);
            inference_trace_attempt.record_started(&request);
            let client =
                ApiMessagesClient::new(transport, client_setup.api_provider, client_setup.api_auth)
                    .with_telemetry(Some(request_telemetry), Some(sse_telemetry));
            let stream_result = client.stream_request(request, options).await;

            match stream_result {
                Ok(stream) => {
                    let (stream, _) = map_response_stream(
                        stream,
                        request_session_telemetry,
                        inference_trace_attempt,
                        Arc::clone(&self.client.state.provider),
                    );
                    return Ok(stream);
                }
                Err(ApiError::Transport(
                    unauthorized_transport @ TransportError::Http { status, .. },
                )) if status == StatusCode::UNAUTHORIZED => {
                    let response_debug_context =
                        extract_response_debug_context(&unauthorized_transport);
                    inference_trace_attempt.record_failed(
                        &unauthorized_transport,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    pending_retry = PendingUnauthorizedRetry::from_recovery(
                        handle_unauthorized(
                            unauthorized_transport,
                            &mut auth_recovery,
                            session_telemetry,
                            &self.client.state.provider,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(err) => {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let err = self.client.state.provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &err,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    return Err(err);
                }
            }
        }
    }

    /// Streams a turn via the Anthropic Messages schema on Amazon Bedrock.
    ///
    /// The request body is built by [`ModelClient::build_messages_request`] —
    /// the same builder the direct-Anthropic path uses. Bedrock's two body
    /// differences (`model` is URL-path scoped rather than a body field, and the
    /// API version travels in the body instead of the `anthropic-version`
    /// header) are applied inside `forall-api`'s Bedrock endpoint, so nothing
    /// here has to know about them and the two wires cannot drift.
    ///
    /// Everything else mirrors [`Self::stream_messages_api`]: the same
    /// 401-recovery loop and the same [`map_response_stream`] adapter, because
    /// the Bedrock transport feeds the identical Messages translator.
    #[instrument(
        name = "model_client.stream_bedrock_messages_api",
        level = "info",
        skip_all,
        fields(
            model = %model_info.slug,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "bedrock_event_stream",
            http.method = "POST",
            api.path = "model/invoke-with-response-stream",
            turn.has_metadata_header = responses_metadata.has_turn_metadata()
        )
    )]
    async fn stream_bedrock_messages_api(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        responses_metadata: &ForallResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let auth_manager = self.client.state.provider.auth_manager();
        let mut auth_recovery = auth_manager
            .as_ref()
            .map(AuthManager::unauthorized_recovery);
        let mut pending_retry = PendingUnauthorizedRetry::default();
        loop {
            let client_setup = self.client.current_client_setup().await?;
            let transport = ReqwestTransport::new(build_reqwest_client());
            let request_auth_context = AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                pending_retry,
            );
            let (request_telemetry, sse_telemetry) = Self::build_streaming_telemetry(
                session_telemetry,
                request_auth_context,
                RequestRouteTelemetry::for_endpoint(BEDROCK_MESSAGES_ENDPOINT),
                self.client.state.auth_env_telemetry.clone(),
            );
            let mut options = ApiBedrockMessagesOptions {
                extra_headers: ApiHeaderMap::new(),
                // Request compression is a Forall-backend affordance; Bedrock
                // gets plain JSON. It would also break SigV4, which signs the
                // body bytes that go on the wire.
                compression: Compression::None,
                // Freeform tools are advertised as single-property object
                // tools; the translator needs their names to unwrap the
                // resulting calls back into `CustomToolCall`.
                freeform_tool_names: forall_tools::freeform_tool_names(&prompt.tools),
            };

            let request = self
                .client
                .build_messages_request(prompt, model_info, effort.clone())?;
            let resolved_effort = model_info
                .supports_reasoning_summaries
                .then(|| ModelClient::resolve_reasoning_effort(effort.clone(), model_info))
                .flatten();
            let request_session_telemetry = session_telemetry
                .clone()
                .with_inference_request(/*service_tier*/ None, resolved_effort.as_ref());
            let inference_trace_attempt = inference_trace.start_attempt();
            inference_trace_attempt.add_request_headers(&mut options.extra_headers);
            inference_trace_attempt.record_started(&request);
            let client = ApiBedrockMessagesClient::new(
                transport,
                client_setup.api_provider,
                client_setup.api_auth,
            )
            .with_telemetry(Some(request_telemetry), Some(sse_telemetry));
            let stream_result = client
                .stream_request(&model_info.slug, request, options)
                .await;

            match stream_result {
                Ok(stream) => {
                    let (stream, _) = map_response_stream(
                        stream,
                        request_session_telemetry,
                        inference_trace_attempt,
                        Arc::clone(&self.client.state.provider),
                    );
                    return Ok(stream);
                }
                Err(ApiError::Transport(
                    unauthorized_transport @ TransportError::Http { status, .. },
                )) if status == StatusCode::UNAUTHORIZED => {
                    let response_debug_context =
                        extract_response_debug_context(&unauthorized_transport);
                    inference_trace_attempt.record_failed(
                        &unauthorized_transport,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    pending_retry = PendingUnauthorizedRetry::from_recovery(
                        handle_unauthorized(
                            unauthorized_transport,
                            &mut auth_recovery,
                            session_telemetry,
                            &self.client.state.provider,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(err) => {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let err = self.client.state.provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &err,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    return Err(err);
                }
            }
        }
    }

    /// Streams a turn via the Responses API over WebSocket transport.
    #[allow(clippy::too_many_arguments)]
    #[instrument(
        name = "model_client.stream_responses_websocket",
        level = "info",
        skip_all,
        fields(
            model = %model_info.slug,
            wire_api = %self.client.state.provider.info().wire_api,
            transport = "responses_websocket",
            api.path = "responses",
            turn.has_metadata_header = responses_metadata.has_turn_metadata(),
            websocket.warmup = warmup
        )
    )]
    async fn stream_responses_websocket(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &ForallResponsesMetadata,
        warmup: bool,
        request_trace: Option<W3cTraceContext>,
        inference_trace: &InferenceTraceContext,
    ) -> Result<WebsocketStreamOutcome> {
        let auth_manager = self.client.state.provider.auth_manager();

        let mut auth_recovery = auth_manager
            .as_ref()
            .map(AuthManager::unauthorized_recovery);
        let mut pending_retry = PendingUnauthorizedRetry::default();
        loop {
            let client_setup = self.client.current_client_setup().await?;
            let request_auth_context = AuthRequestTelemetryContext::new(
                client_setup.auth.as_ref().map(ForallAuth::auth_mode),
                client_setup.api_auth.as_ref(),
                pending_retry,
            );
            let request = self.client.build_responses_request(
                &client_setup.api_provider,
                prompt,
                model_info,
                effort.clone(),
                summary,
                service_tier.clone(),
                responses_metadata,
            )?;
            let request_session_telemetry = if warmup {
                // `generate=false` prewarm is connection setup, not an inference request.
                session_telemetry.clone()
            } else {
                session_telemetry_for_request(session_telemetry, &request)
            };
            let mut client_metadata = self
                .client
                .build_ws_client_metadata(responses_metadata, model_info.use_responses_lite);
            if let Some(turn_state) = self.turn_state.get() {
                client_metadata.insert(X_CODEX_TURN_STATE_HEADER.to_string(), turn_state.clone());
            }
            let mut ws_payload = ResponseCreateWsRequest {
                client_metadata: response_create_client_metadata(
                    Some(client_metadata),
                    request_trace.as_ref(),
                ),
                ..ResponseCreateWsRequest::from(&request)
            };
            if warmup {
                ws_payload.generate = Some(false);
            }

            match self
                .websocket_connection(WebsocketConnectParams {
                    session_telemetry,
                    api_provider: client_setup.api_provider,
                    api_auth: client_setup.api_auth,
                    responses_metadata,
                    auth_context: request_auth_context,
                    request_route_telemetry: RequestRouteTelemetry::for_endpoint(
                        RESPONSES_ENDPOINT,
                    ),
                })
                .await
            {
                Ok(_) => {}
                Err(ApiError::Transport(TransportError::Http { status, .. }))
                    if status == StatusCode::UPGRADE_REQUIRED =>
                {
                    return Ok(WebsocketStreamOutcome::FallbackToHttp);
                }
                Err(ApiError::Transport(
                    unauthorized_transport @ TransportError::Http { status, .. },
                )) if status == StatusCode::UNAUTHORIZED => {
                    pending_retry = PendingUnauthorizedRetry::from_recovery(
                        handle_unauthorized(
                            unauthorized_transport,
                            &mut auth_recovery,
                            session_telemetry,
                            &self.client.state.provider,
                        )
                        .await?,
                    );
                    continue;
                }
                Err(err) => return Err(self.client.state.provider.map_api_error(err)),
            }

            let (mut ws_request, previous_response_id_from_untraced_warmup) =
                self.prepare_websocket_request(ws_payload, &request);
            let inference_trace_attempt = if warmup {
                // Prewarm sends `generate=false`; it is connection setup, not a
                // model inference attempt that should appear in rollout traces.
                InferenceTraceAttempt::disabled()
            } else {
                inference_trace.start_attempt()
            };
            stamp_ws_stream_request_start_ms(&mut ws_request);
            let ResponsesWsRequest::ResponseCreate(ws_payload) = &mut ws_request;
            let store = ws_payload.store;
            self.client
                .prepare_response_items_for_request(&mut ws_payload.input, store);
            if previous_response_id_from_untraced_warmup {
                // The transport can reuse an untraced warmup response id and omit the
                // already-sent input, but rollout replay needs the logical model-visible
                // request rather than the compressed websocket delta.
                inference_trace_attempt.record_started(&request);
            } else {
                inference_trace_attempt.record_started(&ws_request);
            }
            self.websocket_session.last_request = Some(request);
            self.websocket_session.last_response_from_untraced_warmup = warmup;
            let websocket_connection =
                self.websocket_session.connection.as_ref().ok_or_else(|| {
                    self.client.state.provider.map_api_error(ApiError::Stream(
                        "websocket connection is unavailable".to_string(),
                    ))
                })?;
            let stream_result = websocket_connection
                .stream_request(
                    ws_request,
                    self.websocket_session.connection_reused(),
                    Some(Arc::clone(&self.turn_state)),
                )
                .await
                .map_err(|err| {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let err = self.client.state.provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &err,
                        response_debug_context.request_id.as_deref(),
                        /*output_items*/ &[],
                    );
                    err
                })?;
            let (stream, last_request_rx) = map_response_stream(
                stream_result,
                request_session_telemetry,
                inference_trace_attempt,
                Arc::clone(&self.client.state.provider),
            );
            self.websocket_session.last_response_rx = Some(last_request_rx);
            return Ok(WebsocketStreamOutcome::Stream(stream));
        }
    }

    /// Builds request and SSE telemetry for streaming API calls.
    fn build_streaming_telemetry(
        session_telemetry: &SessionTelemetry,
        auth_context: AuthRequestTelemetryContext,
        request_route_telemetry: RequestRouteTelemetry,
        auth_env_telemetry: AuthEnvTelemetry,
    ) -> (Arc<dyn RequestTelemetry>, Arc<dyn SseTelemetry>) {
        let telemetry = Arc::new(ApiTelemetry::new(
            session_telemetry.clone(),
            auth_context,
            request_route_telemetry,
            auth_env_telemetry,
        ));
        let request_telemetry: Arc<dyn RequestTelemetry> = telemetry.clone();
        let sse_telemetry: Arc<dyn SseTelemetry> = telemetry;
        (request_telemetry, sse_telemetry)
    }

    /// Builds telemetry for the Responses API WebSocket transport.
    fn build_websocket_telemetry(
        session_telemetry: &SessionTelemetry,
        auth_context: AuthRequestTelemetryContext,
        request_route_telemetry: RequestRouteTelemetry,
        auth_env_telemetry: AuthEnvTelemetry,
    ) -> Arc<dyn WebsocketTelemetry> {
        let telemetry = Arc::new(ApiTelemetry::new(
            session_telemetry.clone(),
            auth_context,
            request_route_telemetry,
            auth_env_telemetry,
        ));
        let websocket_telemetry: Arc<dyn WebsocketTelemetry> = telemetry;
        websocket_telemetry
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prewarm_websocket(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &ForallResponsesMetadata,
    ) -> Result<()> {
        if !self.client.responses_websocket_enabled() {
            return Ok(());
        }
        if self.websocket_session.last_request.is_some() {
            return Ok(());
        }

        let disabled_trace = InferenceTraceContext::disabled();
        match self
            .stream_responses_websocket(
                prompt,
                model_info,
                session_telemetry,
                effort,
                summary,
                service_tier,
                responses_metadata,
                /*warmup*/ true,
                current_span_w3c_trace_context(),
                &disabled_trace,
            )
            .await
        {
            Ok(WebsocketStreamOutcome::Stream(mut stream)) => {
                // Wait for the v2 warmup request to complete before sending the first turn request.
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(ResponseEvent::Completed { .. }) => break,
                        Err(err) => return Err(err),
                        _ => {}
                    }
                }
                Ok(())
            }
            Ok(WebsocketStreamOutcome::FallbackToHttp) => {
                self.try_switch_fallback_transport(session_telemetry, model_info);
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Streams a single model request within the current turn.
    ///
    /// The caller is responsible for passing per-turn settings explicitly (model selection,
    /// reasoning settings, telemetry context, and turn metadata). This method will prefer the
    /// Responses WebSocket transport when the provider supports it and it remains healthy, and will
    /// fall back to the HTTP Responses API transport otherwise. The trace context may be enabled or
    /// disabled, but is always explicit so transport paths do not need separate trace/no-trace
    /// branches.
    pub async fn stream(
        &mut self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &ForallResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        let wire_api = self.client.state.provider.info().wire_api;
        match wire_api {
            WireApi::Responses => {
                if self.client.responses_websocket_enabled() {
                    let request_trace = current_span_w3c_trace_context();
                    match self
                        .stream_responses_websocket(
                            prompt,
                            model_info,
                            session_telemetry,
                            effort.clone(),
                            summary,
                            service_tier.clone(),
                            responses_metadata,
                            /*warmup*/ false,
                            request_trace,
                            inference_trace,
                        )
                        .await?
                    {
                        WebsocketStreamOutcome::Stream(stream) => return Ok(stream),
                        WebsocketStreamOutcome::FallbackToHttp => {
                            self.try_switch_fallback_transport(session_telemetry, model_info);
                        }
                    }
                }

                self.stream_responses_api(
                    prompt,
                    model_info,
                    session_telemetry,
                    effort,
                    summary,
                    service_tier,
                    responses_metadata,
                    inference_trace,
                )
                .await
            }
            // The Messages API is HTTP-only: Messages providers set
            // `supports_websockets: false`, so there is no websocket
            // preference step here. `summary` and `service_tier` are
            // Responses-only concepts and are not forwarded.
            WireApi::Messages => {
                self.stream_messages_api(
                    prompt,
                    model_info,
                    session_telemetry,
                    effort,
                    responses_metadata,
                    inference_trace,
                )
                .await
            }
            // Gemini is likewise HTTP-only (`supports_websockets: false`), and
            // `summary`/`service_tier` have no equivalents on this wire.
            WireApi::Gemini => {
                self.stream_gemini_api(
                    prompt,
                    model_info,
                    session_telemetry,
                    effort,
                    responses_metadata,
                    inference_trace,
                )
                .await
            }
            // Claude on Bedrock: the same Messages request/response schema as
            // `WireApi::Messages`, reached over a model-scoped invoke path and
            // AWS event-stream framing instead of `/messages` and SSE.
            WireApi::BedrockMessages => {
                self.stream_bedrock_messages_api(
                    prompt,
                    model_info,
                    session_telemetry,
                    effort,
                    responses_metadata,
                    inference_trace,
                )
                .await
            }
        }
    }

    /// Permanently disables WebSockets for this Forall session and resets WebSocket state.
    ///
    /// This is used after exhausting the provider retry budget, to force subsequent requests onto
    /// the HTTP transport.
    ///
    /// Returns `true` if this call activated fallback, or `false` if fallback was already active.
    pub(crate) fn try_switch_fallback_transport(
        &mut self,
        session_telemetry: &SessionTelemetry,
        model_info: &ModelInfo,
    ) -> bool {
        let activated = self
            .client
            .force_http_fallback(session_telemetry, model_info);
        self.websocket_session = WebsocketSession::default();
        activated
    }
}

/// Stamp a ResponsesWsRequest with the current time.
///
/// Meant to be called just before sending the request over the socket, to capture realistic
/// transport timing.
fn stamp_ws_stream_request_start_ms(request: &mut ResponsesWsRequest) {
    let ResponsesWsRequest::ResponseCreate(payload) = request;
    payload
        .client_metadata
        .get_or_insert_with(HashMap::new)
        .insert(
            X_CODEX_WS_STREAM_REQUEST_START_MS_CLIENT_METADATA_KEY.to_string(),
            crate::turn_timing::now_unix_timestamp_ms().to_string(),
        );
}

/// Builds the extra headers attached to Responses API requests.
///
/// These headers implement Forall-specific conventions:
///
/// - `x-forall-beta-features`: comma-separated beta feature keys enabled for the session.
/// - `x-forall-turn-state`: sticky routing token captured earlier in the turn.
fn build_responses_headers(
    beta_features_header: Option<&str>,
    turn_state: Option<&Arc<OnceLock<String>>>,
) -> ApiHeaderMap {
    let mut headers = ApiHeaderMap::new();
    if let Some(value) = beta_features_header
        && !value.is_empty()
        && let Ok(header_value) = HeaderValue::from_str(value)
    {
        headers.insert("x-forall-beta-features", header_value);
    }
    if let Some(turn_state) = turn_state
        && let Some(state) = turn_state.get()
        && let Ok(header_value) = HeaderValue::from_str(state)
    {
        headers.insert(X_CODEX_TURN_STATE_HEADER, header_value);
    }
    headers
}

fn add_responses_lite_header(headers: &mut ApiHeaderMap, use_responses_lite: bool) {
    if use_responses_lite {
        headers.insert(
            X_OPENAI_INTERNAL_CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
    }
}

// ---------------------------------------------------------------------------
// Messages API history translation
// ---------------------------------------------------------------------------

/// Translates the flat `ResponseItem` history into the Messages API
/// `messages` array.
///
/// Consecutive same-role items are merged into one message: the Messages API
/// enforces strict user/assistant alternation on `messages` entries, and
/// merging blocks into a single turn is the lossless way to satisfy it. A
/// translated history that would start with an assistant turn (or an empty
/// history) gets a placeholder user text message, because the API also
/// requires a non-empty array whose first entry is a user turn.
fn build_messages_history(input: &[ResponseItem]) -> Vec<MessagesMessage> {
    let mut messages: Vec<MessagesMessage> = Vec::new();
    for item in input {
        let Some((role, blocks)) = messages_blocks_for_item(item) else {
            continue;
        };
        match messages.last_mut() {
            Some(last) if last.role == role => last.content.extend(blocks),
            _ => messages.push(MessagesMessage {
                role,
                content: blocks,
            }),
        }
    }
    if !matches!(messages.first(), Some(first) if first.role == MessagesRole::User) {
        messages.insert(
            0,
            MessagesMessage {
                role: MessagesRole::User,
                content: vec![MessagesContentBlock::Text {
                    text: MESSAGES_PLACEHOLDER_USER_TEXT.to_string(),
                    cache_control: None,
                }],
            },
        );
    }
    messages
}

/// Translates one history item into content blocks, or `None` when the item
/// has no Messages representation (skips are logged).
fn messages_blocks_for_item(
    item: &ResponseItem,
) -> Option<(MessagesRole, Vec<MessagesContentBlock>)> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let role = messages_role_for(role);
            let blocks: Vec<MessagesContentBlock> = content
                .iter()
                .map(|content_item| match content_item {
                    ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                        MessagesContentBlock::Text {
                            text: text.clone(),
                            cache_control: None,
                        }
                    }
                    ContentItem::InputImage { image_url, .. } => MessagesContentBlock::Image {
                        source: messages_image_source(image_url),
                        cache_control: None,
                    },
                })
                .collect();
            (!blocks.is_empty()).then_some((role, blocks))
        }
        ResponseItem::Reasoning {
            encrypted_content, ..
        } => {
            let Some(block) = encrypted_content
                .as_deref()
                .and_then(decode_thinking_envelope)
            else {
                // Anthropic hard-rejects replayed thinking blocks whose
                // signature does not match, so a reasoning item without a
                // decodable envelope (e.g. OpenAI encrypted reasoning from a
                // mid-session model switch) must be dropped, not guessed at.
                warn!("skipping reasoning item without a decodable thinking envelope");
                return None;
            };
            Some((MessagesRole::Assistant, vec![block]))
        }
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => Some((
            MessagesRole::Assistant,
            vec![MessagesContentBlock::ToolUse {
                id: call_id.clone(),
                name: name.clone(),
                input: tool_use_input_from_arguments(arguments),
                cache_control: None,
            }],
        )),
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
            ..
        } => Some((
            MessagesRole::Assistant,
            vec![MessagesContentBlock::ToolUse {
                id: call_id.clone(),
                name: name.clone(),
                input: tool_use_input_from_arguments(input),
                cache_control: None,
            }],
        )),
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        }
        | ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            let (content, is_error) = tool_result_content(output);
            Some((
                MessagesRole::User,
                vec![MessagesContentBlock::ToolResult {
                    tool_use_id: call_id.clone(),
                    content,
                    is_error,
                    cache_control: None,
                }],
            ))
        }
        ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => {
            warn!(
                "skipping history item with no Messages API representation: {}",
                unmappable_item_kind(item)
            );
            None
        }
    }
}

/// Serde-tag label used for skip logs; only meaningful for the unmappable
/// variants of [`messages_blocks_for_item`].
fn unmappable_item_kind(item: &ResponseItem) -> &'static str {
    match item {
        ResponseItem::AgentMessage { .. } => "agent_message",
        ResponseItem::LocalShellCall { .. } => "local_shell_call",
        ResponseItem::ToolSearchCall { .. } => "tool_search_call",
        ResponseItem::ToolSearchOutput { .. } => "tool_search_output",
        ResponseItem::WebSearchCall { .. } => "web_search_call",
        ResponseItem::ImageGenerationCall { .. } => "image_generation_call",
        ResponseItem::Compaction { .. } => "compaction",
        ResponseItem::CompactionTrigger { .. } => "compaction_trigger",
        ResponseItem::ContextCompaction { .. } => "context_compaction",
        _ => "other",
    }
}

/// History roles are "user"/"assistant"; anything unexpected is delivered as
/// user content so the model still sees it.
fn messages_role_for(role: &str) -> MessagesRole {
    if role == "assistant" {
        MessagesRole::Assistant
    } else {
        MessagesRole::User
    }
}

/// Splits a data URL (`data:<media_type>;base64,<data>`) into a base64 image
/// source; any other URL is passed through as a URL source.
fn messages_image_source(image_url: &str) -> MessagesImageSource {
    if let Some(rest) = image_url.strip_prefix("data:")
        && let Some((media_type, data)) = rest.split_once(";base64,")
    {
        return MessagesImageSource::Base64 {
            media_type: media_type.to_string(),
            data: data.to_string(),
        };
    }
    MessagesImageSource::Url {
        url: image_url.to_string(),
    }
}

/// Decomposes the stage-1 round-trip envelope stored in
/// `ResponseItem::Reasoning::encrypted_content` back into a thinking block.
///
/// The envelope is base64 over JSON: `{"thinking", "signature"}` becomes a
/// signed `thinking` block, `{"redacted"}` becomes `redacted_thinking`.
/// Returns `None` for anything that does not decode to one of those shapes.
fn decode_thinking_envelope(encrypted_content: &str) -> Option<MessagesContentBlock> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encrypted_content)
        .ok()?;
    let envelope: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    if let Some(redacted) = envelope.get("redacted").and_then(serde_json::Value::as_str) {
        return Some(MessagesContentBlock::RedactedThinking {
            data: redacted.to_string(),
        });
    }
    let thinking = envelope
        .get("thinking")
        .and_then(serde_json::Value::as_str)?;
    let signature = envelope
        .get("signature")
        .and_then(serde_json::Value::as_str)?;
    Some(MessagesContentBlock::Thinking {
        thinking: thinking.to_string(),
        signature: signature.to_string(),
    })
}

/// Parses the raw tool-call arguments string back into the JSON value the
/// Messages API expects for `tool_use.input`. Arguments that are not valid
/// JSON are preserved under a `_raw` key rather than dropped.
fn tool_use_input_from_arguments(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| serde_json::json!({ "_raw": arguments }))
}

/// Converts a tool-call output payload into `tool_result` content blocks plus
/// the `is_error` flag.
///
/// `EncryptedContent` items have no Messages representation and are replaced
/// by a placeholder text block with a warning.
fn tool_result_content(
    output: &FunctionCallOutputPayload,
) -> (Vec<MessagesToolResultContent>, Option<bool>) {
    let content = match &output.body {
        FunctionCallOutputBody::Text(text) => {
            vec![MessagesToolResultContent::Text { text: text.clone() }]
        }
        FunctionCallOutputBody::ContentItems(items) => items
            .iter()
            .map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => {
                    MessagesToolResultContent::Text { text: text.clone() }
                }
                FunctionCallOutputContentItem::InputImage { image_url, .. } => {
                    MessagesToolResultContent::Image {
                        source: messages_image_source(image_url),
                    }
                }
                FunctionCallOutputContentItem::EncryptedContent { .. } => {
                    warn!(
                        "replacing encrypted tool output content with a placeholder; the Messages API has no encrypted content representation"
                    );
                    MessagesToolResultContent::Text {
                        text: "[encrypted content omitted]".to_string(),
                    }
                }
            })
            .collect(),
    };
    let is_error = matches!(output.success, Some(false)).then_some(true);
    (content, is_error)
}

/// Places the third cache breakpoint on the final content block of the
/// SECOND-most-recent user message.
///
/// Everything before a breakpoint is cached, so marking the previous user
/// turn caches the shared prefix up to and including the last completed turn
/// while leaving the newest (never-before-seen) user turn out of the cache
/// key. With a single user message there is no shared prior turn and no
/// breakpoint is placed.
fn apply_history_cache_breakpoint(messages: &mut [MessagesMessage]) {
    let mut user_indexes = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, message)| message.role == MessagesRole::User)
        .map(|(index, _)| index);
    let _most_recent_user = user_indexes.next();
    let Some(previous_user_index) = user_indexes.next() else {
        return;
    };
    if let Some(message) = messages.get_mut(previous_user_index)
        && let Some(block) = message.content.last_mut()
    {
        attach_cache_control(block);
    }
}

/// Attaches an ephemeral cache breakpoint to a block. Thinking blocks cannot
/// carry `cache_control`; returns whether the breakpoint was placed.
fn attach_cache_control(block: &mut MessagesContentBlock) -> bool {
    match block {
        MessagesContentBlock::Text { cache_control, .. }
        | MessagesContentBlock::Image { cache_control, .. }
        | MessagesContentBlock::ToolUse { cache_control, .. }
        | MessagesContentBlock::ToolResult { cache_control, .. } => {
            *cache_control = Some(CacheControl::Ephemeral);
            true
        }
        MessagesContentBlock::Thinking { .. } | MessagesContentBlock::RedactedThinking { .. } => {
            false
        }
    }
}

/// Maps a resolved reasoning effort onto an extended-thinking token budget.
///
/// `None` effort disables thinking; unknown model-defined efforts get the
/// medium budget rather than silently disabling thinking.
fn thinking_budget_tokens(effort: &ReasoningEffortConfig) -> Option<u32> {
    match effort {
        ReasoningEffortConfig::None => None,
        ReasoningEffortConfig::Minimal | ReasoningEffortConfig::Low => Some(4_000),
        ReasoningEffortConfig::Medium | ReasoningEffortConfig::Custom(_) => Some(10_000),
        ReasoningEffortConfig::High | ReasoningEffortConfig::XHigh => Some(24_000),
    }
}

// ---------------------------------------------------------------------------
// Gemini API history translation
// ---------------------------------------------------------------------------

/// Translates the flat `ResponseItem` history into the Gemini `contents` array.
///
/// Consecutive same-role items are merged into one `Content`. Gemini does not
/// require strict user/model alternation (unlike the Messages API), but merging
/// keeps the payload compact and matches how Gemini itself groups a turn's
/// parts. An empty result gets a placeholder user turn, because `contents` may
/// not be empty.
///
/// `functionResponse` parts need the *function name*, which
/// `ResponseItem::FunctionCallOutput` does not carry — it only has a `call_id`.
/// The name is therefore resolved from the `functionCall` earlier in the same
/// history, which is why this is a single stateful pass rather than a per-item
/// map.
fn build_gemini_contents(input: &[ResponseItem]) -> Vec<GeminiContent> {
    let mut contents: Vec<GeminiContent> = Vec::new();
    let mut call_names: HashMap<String, String> = HashMap::new();
    for item in input {
        let Some((role, parts)) = gemini_parts_for_item(item, &mut call_names) else {
            continue;
        };
        match contents.last_mut() {
            Some(last) if last.role == role => last.parts.extend(parts),
            _ => contents.push(GeminiContent { role, parts }),
        }
    }
    if contents.is_empty() {
        contents.push(GeminiContent {
            role: GeminiRole::User,
            parts: vec![GeminiPart::Text {
                text: GEMINI_PLACEHOLDER_USER_TEXT.to_string(),
            }],
        });
    }
    contents
}

/// Translates one history item into Gemini parts, or `None` when the item has
/// no Gemini representation (skips are logged).
///
/// `call_names` accumulates `call_id -> function name` as `functionCall` items
/// are seen, so their outputs can be labeled; see [`build_gemini_contents`].
fn gemini_parts_for_item(
    item: &ResponseItem,
    call_names: &mut HashMap<String, String>,
) -> Option<(GeminiRole, Vec<GeminiPart>)> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let role = gemini_role_for(role);
            let parts: Vec<GeminiPart> = content
                .iter()
                .filter_map(|content_item| match content_item {
                    ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                        Some(GeminiPart::Text { text: text.clone() })
                    }
                    ContentItem::InputImage { image_url, .. } => gemini_image_part(image_url),
                })
                .collect();
            (!parts.is_empty()).then_some((role, parts))
        }
        ResponseItem::Reasoning {
            encrypted_content, ..
        } => {
            let Some(part) = encrypted_content
                .as_deref()
                .and_then(decode_thought_envelope)
            else {
                // A thought part is only useful on replay when it carries the
                // `thoughtSignature` it was issued with; the envelope is the
                // only place that pairing survives. A reasoning item without a
                // decodable envelope (an unsigned thought, or OpenAI encrypted
                // reasoning left over from a mid-session model switch) is
                // dropped rather than replayed as an unsigned guess.
                warn!("skipping reasoning item without a decodable thought envelope");
                return None;
            };
            Some((GeminiRole::Model, vec![part]))
        }
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            call_names.insert(call_id.clone(), name.clone());
            Some((
                GeminiRole::Model,
                vec![GeminiPart::FunctionCall {
                    function_call: GeminiFunctionCall {
                        name: name.clone(),
                        args: gemini_args_from_arguments(arguments),
                    },
                }],
            ))
        }
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
            ..
        } => {
            call_names.insert(call_id.clone(), name.clone());
            Some((
                GeminiRole::Model,
                vec![GeminiPart::FunctionCall {
                    function_call: GeminiFunctionCall {
                        name: name.clone(),
                        // A freeform tool was advertised as a one-string-property
                        // function, so its payload is re-wrapped into that
                        // property to match the declared schema.
                        args: serde_json::json!({
                            forall_tools::GEMINI_FREEFORM_INPUT_PROPERTY: input,
                        }),
                    },
                }],
            ))
        }
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => {
            let name = gemini_function_name_for_output(call_id, /*name*/ None, call_names)?;
            Some((
                GeminiRole::User,
                vec![GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name,
                        response: gemini_function_response_body(output),
                    },
                }],
            ))
        }
        ResponseItem::CustomToolCallOutput {
            call_id,
            name,
            output,
            ..
        } => {
            let name = gemini_function_name_for_output(call_id, name.as_deref(), call_names)?;
            Some((
                GeminiRole::User,
                vec![GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name,
                        response: gemini_function_response_body(output),
                    },
                }],
            ))
        }
        ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => {
            warn!(
                "skipping history item with no Gemini API representation: {}",
                unmappable_item_kind(item)
            );
            None
        }
    }
}

/// Resolves the function name a `functionResponse` must carry.
///
/// Gemini matches results to calls by name, so an output whose call cannot be
/// resolved is dropped: emitting one with a wrong or empty name would either be
/// rejected or silently attributed to a different tool.
fn gemini_function_name_for_output(
    call_id: &str,
    name: Option<&str>,
    call_names: &HashMap<String, String>,
) -> Option<String> {
    if let Some(name) = call_names.get(call_id) {
        return Some(name.clone());
    }
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        return Some(name.to_string());
    }
    warn!(
        "skipping tool output for call_id {call_id}: no matching functionCall in history to take the function name from"
    );
    None
}

/// History roles are "user"/"assistant"; Gemini names the assistant role
/// `model`, and anything unexpected is delivered as user content so the model
/// still sees it.
fn gemini_role_for(role: &str) -> GeminiRole {
    if role == "assistant" {
        GeminiRole::Model
    } else {
        GeminiRole::User
    }
}

/// Converts an image URL into an `inlineData` part.
///
/// Only `data:` URLs can be represented: Gemini's non-inline alternative
/// (`fileData.fileUri`) accepts only Files-API or GCS URIs, so forwarding an
/// arbitrary HTTP URL would be rejected. Those are dropped with a warning
/// rather than sent as text the model would mistake for content.
fn gemini_image_part(image_url: &str) -> Option<GeminiPart> {
    if let Some(rest) = image_url.strip_prefix("data:")
        && let Some((mime_type, data)) = rest.split_once(";base64,")
    {
        return Some(GeminiPart::InlineData {
            inline_data: forall_api::GeminiBlob {
                mime_type: mime_type.to_string(),
                data: data.to_string(),
            },
        });
    }
    warn!(
        "skipping image with a non-data URL: Gemini inlineData needs base64 bytes and fileData needs a Files API URI"
    );
    None
}

/// Decodes the round-trip envelope stored in
/// `ResponseItem::Reasoning::encrypted_content` back into a thought part.
///
/// The envelope is base64 over JSON `{"text", "thoughtSignature"}`. Returns
/// `None` for anything that does not decode to that shape — notably OpenAI
/// encrypted reasoning and the Anthropic thinking envelope, which must not be
/// replayed onto this wire.
fn decode_thought_envelope(encrypted_content: &str) -> Option<GeminiPart> {
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encrypted_content)
        .ok()?;
    let envelope: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let text = envelope.get("text").and_then(serde_json::Value::as_str)?;
    let signature = envelope
        .get("thoughtSignature")
        .and_then(serde_json::Value::as_str)?;
    Some(GeminiPart::Thought {
        text: text.to_string(),
        thought: true,
        thought_signature: Some(signature.to_string()),
    })
}

/// Parses the raw tool-call arguments string back into the JSON object Gemini
/// expects for `functionCall.args`.
///
/// `args` must be an object on the wire, so a non-object (or unparseable)
/// payload is preserved under a `_raw` key rather than dropped.
fn gemini_args_from_arguments(arguments: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value @ serde_json::Value::Object(_)) => value,
        _ => serde_json::json!({ "_raw": arguments }),
    }
}

/// Converts a tool-call output payload into the `functionResponse.response`
/// object.
///
/// `response` must be a JSON object, so text output is wrapped: successful
/// output under `output`, failures under `error` (Google's documented
/// convention for signalling a tool failure to the model). Image and encrypted
/// content items have no representation and are replaced by placeholders.
fn gemini_function_response_body(output: &FunctionCallOutputPayload) -> serde_json::Value {
    let text = match &output.body {
        FunctionCallOutputBody::Text(text) => text.clone(),
        FunctionCallOutputBody::ContentItems(items) => items
            .iter()
            .map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } => text.clone(),
                FunctionCallOutputContentItem::InputImage { .. } => {
                    warn!(
                        "replacing image tool output with a placeholder; a Gemini functionResponse carries only JSON"
                    );
                    "[image omitted]".to_string()
                }
                FunctionCallOutputContentItem::EncryptedContent { .. } => {
                    warn!(
                        "replacing encrypted tool output content with a placeholder; Gemini has no encrypted content representation"
                    );
                    "[encrypted content omitted]".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    };
    if matches!(output.success, Some(false)) {
        serde_json::json!({ "error": text })
    } else {
        serde_json::json!({ "output": text })
    }
}

/// Maps a resolved reasoning effort onto a Gemini thinking budget.
///
/// `None` effort disables thinking; unknown model-defined efforts get the
/// medium budget rather than silently disabling thinking. The budget is signed
/// because Gemini reserves `-1` for "choose dynamically" — a value this mapping
/// never produces, since core always has a concrete effort to honor.
fn gemini_thinking_budget(effort: &ReasoningEffortConfig) -> Option<i32> {
    match effort {
        ReasoningEffortConfig::None => None,
        ReasoningEffortConfig::Minimal | ReasoningEffortConfig::Low => Some(4_000),
        ReasoningEffortConfig::Medium | ReasoningEffortConfig::Custom(_) => Some(10_000),
        ReasoningEffortConfig::High | ReasoningEffortConfig::XHigh => Some(24_000),
    }
}

const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 1600;
const STREAM_DROPPED_REASON: &str = "response stream dropped before provider terminal event";

fn map_response_stream(
    api_stream: forall_api::ResponseStream,
    session_telemetry: SessionTelemetry,
    inference_trace_attempt: InferenceTraceAttempt,
    provider: SharedModelProvider,
) -> (ResponseStream, oneshot::Receiver<LastResponse>) {
    let forall_api::ResponseStream {
        rx_event,
        upstream_request_id,
    } = api_stream;
    let api_stream = forall_api::ResponseStream {
        rx_event,
        upstream_request_id: None,
    };
    map_response_events(
        upstream_request_id,
        api_stream,
        session_telemetry,
        inference_trace_attempt,
        provider,
    )
}

fn map_response_events<S>(
    upstream_request_id: Option<String>,
    api_stream: S,
    session_telemetry: SessionTelemetry,
    inference_trace_attempt: InferenceTraceAttempt,
    provider: SharedModelProvider,
) -> (ResponseStream, oneshot::Receiver<LastResponse>)
where
    S: futures::Stream<Item = std::result::Result<ResponseEvent, ApiError>>
        + Unpin
        + Send
        + 'static,
{
    let (tx_event, rx_event) =
        mpsc::channel::<Result<ResponseEvent>>(RESPONSE_STREAM_CHANNEL_CAPACITY);
    let (tx_last_response, rx_last_response) = oneshot::channel::<LastResponse>();
    let consumer_dropped = CancellationToken::new();
    let consumer_dropped_for_stream = consumer_dropped.clone();

    tokio::spawn(async move {
        let mut logged_error = false;
        let mut tx_last_response = Some(tx_last_response);
        let mut items_added: Vec<ResponseItem> = Vec::new();
        let mut api_stream = api_stream;
        let upstream_request_id = upstream_request_id.as_deref();
        if let Some(upstream_request_id) = upstream_request_id {
            feedback_tags!(last_model_request_id = upstream_request_id);
        }
        loop {
            let event = tokio::select! {
                _ = consumer_dropped.cancelled() => {
                    inference_trace_attempt.record_cancelled(
                        STREAM_DROPPED_REASON,
                        upstream_request_id,
                        &items_added,
                    );
                    return;
                }
                event = api_stream.next() => event,
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Ok(ResponseEvent::OutputItemDone(item)) => {
                    items_added.push(item.clone());
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemDone(item)))
                        .await
                        .is_err()
                    {
                        inference_trace_attempt.record_cancelled(
                            STREAM_DROPPED_REASON,
                            upstream_request_id,
                            &items_added,
                        );
                        return;
                    }
                }
                Ok(ResponseEvent::Completed {
                    response_id,
                    token_usage,
                    end_turn,
                }) => {
                    feedback_tags!(last_model_response_id = &response_id);
                    if let Some(usage) = &token_usage {
                        session_telemetry.sse_event_completed(
                            usage.input_tokens,
                            usage.output_tokens,
                            Some(usage.cached_input_tokens),
                            Some(usage.reasoning_output_tokens),
                            usage.total_tokens,
                        );
                    }
                    inference_trace_attempt.record_completed(
                        &response_id,
                        upstream_request_id,
                        &token_usage,
                        &items_added,
                    );
                    if let Some(sender) = tx_last_response.take() {
                        let _ = sender.send(LastResponse {
                            response_id: response_id.clone(),
                            items_added: std::mem::take(&mut items_added),
                        });
                    }
                    if tx_event
                        .send(Ok(ResponseEvent::Completed {
                            response_id,
                            token_usage,
                            end_turn,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Ok(event) => {
                    if tx_event.send(Ok(event)).await.is_err() {
                        inference_trace_attempt.record_cancelled(
                            STREAM_DROPPED_REASON,
                            upstream_request_id,
                            &items_added,
                        );
                        return;
                    }
                }
                Err(err) => {
                    let response_debug_context =
                        extract_response_debug_context_from_api_error(&err);
                    let upstream_request_id =
                        upstream_request_id.or(response_debug_context.request_id.as_deref());
                    if let Some(upstream_request_id) = upstream_request_id {
                        feedback_tags!(last_model_request_id = upstream_request_id);
                    }
                    let mapped = provider.map_api_error(err);
                    inference_trace_attempt.record_failed(
                        &mapped,
                        upstream_request_id,
                        &items_added,
                    );
                    if !logged_error {
                        session_telemetry.see_event_completed_failed(&mapped);
                        logged_error = true;
                    }
                    if tx_event.send(Err(mapped)).await.is_err() {
                        return;
                    }
                }
            }
        }
        inference_trace_attempt.record_failed(
            "stream closed before response.completed",
            upstream_request_id,
            &items_added,
        );
    });

    (
        ResponseStream {
            rx_event,
            consumer_dropped: consumer_dropped_for_stream,
        },
        rx_last_response,
    )
}

/// Handles a 401 response by optionally refreshing ChatGPT tokens once.
///
/// When refresh succeeds, the caller should retry the API call; otherwise
/// the mapped `ForallErr` is returned to the caller.
#[derive(Clone, Copy, Debug)]
struct UnauthorizedRecoveryExecution {
    mode: &'static str,
    phase: &'static str,
}

#[derive(Clone, Copy, Debug, Default)]
struct PendingUnauthorizedRetry {
    retry_after_unauthorized: bool,
    recovery_mode: Option<&'static str>,
    recovery_phase: Option<&'static str>,
}

impl PendingUnauthorizedRetry {
    fn from_recovery(recovery: UnauthorizedRecoveryExecution) -> Self {
        Self {
            retry_after_unauthorized: true,
            recovery_mode: Some(recovery.mode),
            recovery_phase: Some(recovery.phase),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AuthRequestTelemetryContext {
    auth_mode: Option<&'static str>,
    auth_header_attached: bool,
    auth_header_name: Option<&'static str>,
    retry_after_unauthorized: bool,
    recovery_mode: Option<&'static str>,
    recovery_phase: Option<&'static str>,
}

impl AuthRequestTelemetryContext {
    fn new(
        auth_mode: Option<AuthMode>,
        api_auth: &dyn AuthProvider,
        retry: PendingUnauthorizedRetry,
    ) -> Self {
        let auth_telemetry = auth_header_telemetry(api_auth);
        Self {
            auth_mode: auth_mode.map(|mode| match mode {
                AuthMode::ApiKey | AuthMode::BedrockApiKey => "ApiKey",
                AuthMode::Oauth => "Oauth",
            }),
            auth_header_attached: auth_telemetry.attached,
            auth_header_name: auth_telemetry.name,
            retry_after_unauthorized: retry.retry_after_unauthorized,
            recovery_mode: retry.recovery_mode,
            recovery_phase: retry.recovery_phase,
        }
    }
}

struct WebsocketConnectParams<'a> {
    session_telemetry: &'a SessionTelemetry,
    api_provider: forall_api::Provider,
    api_auth: SharedAuthProvider,
    responses_metadata: &'a ForallResponsesMetadata,
    auth_context: AuthRequestTelemetryContext,
    request_route_telemetry: RequestRouteTelemetry,
}

async fn handle_unauthorized(
    transport: TransportError,
    auth_recovery: &mut Option<UnauthorizedRecovery>,
    session_telemetry: &SessionTelemetry,
    provider: &SharedModelProvider,
) -> Result<UnauthorizedRecoveryExecution> {
    let debug = extract_response_debug_context(&transport);
    if let Some(recovery) = auth_recovery
        && recovery.has_next()
    {
        let mode = recovery.mode_name();
        let phase = recovery.step_name();
        return match recovery.next().await {
            Ok(step_result) => {
                session_telemetry.record_auth_recovery(
                    mode,
                    phase,
                    "recovery_succeeded",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                    /*recovery_reason*/ None,
                    step_result.auth_state_changed(),
                );
                emit_feedback_auth_recovery_tags(
                    mode,
                    phase,
                    "recovery_succeeded",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                );
                Ok(UnauthorizedRecoveryExecution { mode, phase })
            }
            Err(RefreshTokenError::Permanent(failed)) => {
                session_telemetry.record_auth_recovery(
                    mode,
                    phase,
                    "recovery_failed_permanent",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                    /*recovery_reason*/ None,
                    /*auth_state_changed*/ None,
                );
                emit_feedback_auth_recovery_tags(
                    mode,
                    phase,
                    "recovery_failed_permanent",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                );
                Err(ForallErr::RefreshTokenFailed(failed))
            }
            Err(RefreshTokenError::Transient(other)) => {
                session_telemetry.record_auth_recovery(
                    mode,
                    phase,
                    "recovery_failed_transient",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                    /*recovery_reason*/ None,
                    /*auth_state_changed*/ None,
                );
                emit_feedback_auth_recovery_tags(
                    mode,
                    phase,
                    "recovery_failed_transient",
                    debug.request_id.as_deref(),
                    debug.cf_ray.as_deref(),
                    debug.auth_error.as_deref(),
                    debug.auth_error_code.as_deref(),
                );
                Err(ForallErr::Io(other))
            }
        };
    }

    let (mode, phase, recovery_reason) = match auth_recovery.as_ref() {
        Some(recovery) => (
            recovery.mode_name(),
            recovery.step_name(),
            Some(recovery.unavailable_reason()),
        ),
        None => ("none", "none", Some("auth_manager_missing")),
    };
    session_telemetry.record_auth_recovery(
        mode,
        phase,
        "recovery_not_run",
        debug.request_id.as_deref(),
        debug.cf_ray.as_deref(),
        debug.auth_error.as_deref(),
        debug.auth_error_code.as_deref(),
        recovery_reason,
        /*auth_state_changed*/ None,
    );
    emit_feedback_auth_recovery_tags(
        mode,
        phase,
        "recovery_not_run",
        debug.request_id.as_deref(),
        debug.cf_ray.as_deref(),
        debug.auth_error.as_deref(),
        debug.auth_error_code.as_deref(),
    );

    Err(provider.map_api_error(ApiError::Transport(transport)))
}

fn api_error_http_status(error: &ApiError) -> Option<u16> {
    match error {
        ApiError::Transport(TransportError::Http { status, .. }) => Some(status.as_u16()),
        _ => None,
    }
}

struct ApiTelemetry {
    session_telemetry: SessionTelemetry,
    auth_context: AuthRequestTelemetryContext,
    request_route_telemetry: RequestRouteTelemetry,
    auth_env_telemetry: AuthEnvTelemetry,
}

impl ApiTelemetry {
    fn new(
        session_telemetry: SessionTelemetry,
        auth_context: AuthRequestTelemetryContext,
        request_route_telemetry: RequestRouteTelemetry,
        auth_env_telemetry: AuthEnvTelemetry,
    ) -> Self {
        Self {
            session_telemetry,
            auth_context,
            request_route_telemetry,
            auth_env_telemetry,
        }
    }
}

impl RequestTelemetry for ApiTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<HttpStatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let error_message = error.map(telemetry_transport_error_message);
        let status = status.map(|s| s.as_u16());
        let debug = error
            .map(extract_response_debug_context)
            .unwrap_or_default();
        self.session_telemetry.record_api_request(
            attempt,
            status,
            error_message.as_deref(),
            duration,
            self.auth_context.auth_header_attached,
            self.auth_context.auth_header_name,
            self.auth_context.retry_after_unauthorized,
            self.auth_context.recovery_mode,
            self.auth_context.recovery_phase,
            self.request_route_telemetry.endpoint,
            debug.request_id.as_deref(),
            debug.cf_ray.as_deref(),
            debug.auth_error.as_deref(),
            debug.auth_error_code.as_deref(),
        );
        emit_feedback_request_tags_with_auth_env(
            &FeedbackRequestTags {
                endpoint: self.request_route_telemetry.endpoint,
                auth_header_attached: self.auth_context.auth_header_attached,
                auth_header_name: self.auth_context.auth_header_name,
                auth_mode: self.auth_context.auth_mode,
                auth_retry_after_unauthorized: Some(self.auth_context.retry_after_unauthorized),
                auth_recovery_mode: self.auth_context.recovery_mode,
                auth_recovery_phase: self.auth_context.recovery_phase,
                auth_connection_reused: None,
                auth_request_id: debug.request_id.as_deref(),
                auth_cf_ray: debug.cf_ray.as_deref(),
                auth_error: debug.auth_error.as_deref(),
                auth_error_code: debug.auth_error_code.as_deref(),
                auth_recovery_followup_success: self
                    .auth_context
                    .retry_after_unauthorized
                    .then_some(error.is_none()),
                auth_recovery_followup_status: self
                    .auth_context
                    .retry_after_unauthorized
                    .then_some(status)
                    .flatten(),
            },
            &self.auth_env_telemetry,
        );
    }
}

impl SseTelemetry for ApiTelemetry {
    fn on_sse_poll(
        &self,
        result: &std::result::Result<
            Option<std::result::Result<Event, EventStreamError<TransportError>>>,
            tokio::time::error::Elapsed,
        >,
        duration: Duration,
    ) {
        self.session_telemetry.log_sse_event(result, duration);
    }
}

impl WebsocketTelemetry for ApiTelemetry {
    fn on_ws_request(&self, duration: Duration, error: Option<&ApiError>, connection_reused: bool) {
        let error_message = error.map(telemetry_api_error_message);
        let status = error.and_then(api_error_http_status);
        let debug = error
            .map(extract_response_debug_context_from_api_error)
            .unwrap_or_default();
        self.session_telemetry.record_websocket_request(
            duration,
            error_message.as_deref(),
            connection_reused,
        );
        emit_feedback_request_tags_with_auth_env(
            &FeedbackRequestTags {
                endpoint: self.request_route_telemetry.endpoint,
                auth_header_attached: self.auth_context.auth_header_attached,
                auth_header_name: self.auth_context.auth_header_name,
                auth_mode: self.auth_context.auth_mode,
                auth_retry_after_unauthorized: Some(self.auth_context.retry_after_unauthorized),
                auth_recovery_mode: self.auth_context.recovery_mode,
                auth_recovery_phase: self.auth_context.recovery_phase,
                auth_connection_reused: Some(connection_reused),
                auth_request_id: debug.request_id.as_deref(),
                auth_cf_ray: debug.cf_ray.as_deref(),
                auth_error: debug.auth_error.as_deref(),
                auth_error_code: debug.auth_error_code.as_deref(),
                auth_recovery_followup_success: self
                    .auth_context
                    .retry_after_unauthorized
                    .then_some(error.is_none()),
                auth_recovery_followup_status: self
                    .auth_context
                    .retry_after_unauthorized
                    .then_some(status)
                    .flatten(),
            },
            &self.auth_env_telemetry,
        );
    }

    fn on_ws_event(
        &self,
        result: &std::result::Result<Option<std::result::Result<Message, Error>>, ApiError>,
        duration: Duration,
    ) {
        self.session_telemetry
            .record_websocket_event(result, duration);
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
