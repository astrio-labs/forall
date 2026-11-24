use super::AuthRequestTelemetryContext;
use super::ModelClient;
use super::PendingUnauthorizedRetry;
use super::UnauthorizedRecoveryExecution;
use super::X_CODEX_INSTALLATION_ID_HEADER;
use super::X_CODEX_PARENT_THREAD_ID_HEADER;
use super::X_CODEX_TURN_METADATA_HEADER;
use super::X_CODEX_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::AttestationContext;
use crate::AttestationProvider;
use crate::GenerateAttestationFuture;
use crate::responses_metadata::ForallResponsesMetadata;
use crate::test_support::TestForallResponsesRequestKind;
use crate::test_support::responses_metadata as test_responses_metadata;
use forall_api::ApiError;
use forall_api::ResponseEvent;
use forall_api::TransportError;
use forall_app_server_protocol::AuthMode;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_model_provider::BearerAuthProvider;
use forall_model_provider::SharedModelProvider;
use forall_model_provider::create_model_provider;
use forall_model_provider_info::ModelProviderInfo;
use forall_model_provider_info::WireApi;
use forall_model_provider_info::create_oss_provider_with_base_url;
use forall_otel::SessionTelemetry;
use forall_protocol::ThreadId;
use forall_protocol::models::ContentItem;
use forall_protocol::models::ResponseItem;
use forall_protocol::openai_models::ModelInfo;
use forall_protocol::protocol::InternalSessionSource;
use forall_protocol::protocol::SessionSource;
use forall_protocol::protocol::SubAgentSource;
use forall_rollout_trace::ExecutionStatus;
use forall_rollout_trace::InferenceTraceAttempt;
use forall_rollout_trace::InferenceTraceContext;
use forall_rollout_trace::RawTraceEventPayload;
use forall_rollout_trace::RolloutTrace;
use forall_rollout_trace::TraceWriter;
use forall_rollout_trace::replay_bundle;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";

fn test_model_client(session_source: SessionSource) -> ModelClient {
    let provider = create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses);
    let thread_id = ThreadId::new();
    ModelClient::new(
        /*auth_manager*/ None,
        thread_id,
        provider,
        session_source,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*item_ids_enabled*/ false,
        /*attestation_provider*/ None,
    )
}

fn test_model_provider() -> SharedModelProvider {
    test_model_client(SessionSource::Cli).state.provider.clone()
}

fn test_responses_metadata_for_client(
    client: &ModelClient,
    turn_id: Option<&str>,
    window_id: String,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestForallResponsesRequestKind,
) -> ForallResponsesMetadata {
    let thread_id = client.state.thread_id.to_string();
    test_responses_metadata(
        TEST_INSTALLATION_ID,
        &thread_id,
        &thread_id,
        turn_id,
        window_id,
        &client.state.session_source,
        parent_thread_id,
        request_kind,
    )
}

fn test_model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": 272000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

fn test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-test",
        "gpt-test",
        /*account_id*/ None,
        /*account_email*/ None,
        /*auth_mode*/ None,
        "test-originator".to_string(),
        /*log_user_prompts*/ false,
        "test-terminal".to_string(),
        SessionSource::Cli,
    )
}

#[derive(Default)]
struct TagCollectorVisitor {
    tags: BTreeMap<String, String>,
}

impl Visit for TagCollectorVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.tags
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[derive(Clone)]
struct TagCollectorLayer {
    tags: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> Layer<S> for TagCollectorLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if event.metadata().target() != "feedback_tags" {
            return;
        }
        let mut visitor = TagCollectorVisitor::default();
        event.record(&mut visitor);
        self.tags.lock().unwrap().extend(visitor.tags);
    }
}

fn started_inference_attempt(temp: &TempDir) -> anyhow::Result<InferenceTraceAttempt> {
    let writer = Arc::new(TraceWriter::create(
        temp.path(),
        "trace-1".to_string(),
        "rollout-1".to_string(),
        "thread-root".to_string(),
    )?);
    writer.append(RawTraceEventPayload::ThreadStarted {
        thread_id: "thread-root".to_string(),
        agent_path: "/root".to_string(),
        metadata_payload: None,
    })?;
    writer.append(RawTraceEventPayload::ForallTurnStarted {
        forall_turn_id: "turn-1".to_string(),
        thread_id: "thread-root".to_string(),
    })?;

    let inference_trace = InferenceTraceContext::enabled(
        writer,
        "thread-root".to_string(),
        "turn-1".to_string(),
        "gpt-test".to_string(),
        "test-provider".to_string(),
    );
    let attempt = inference_trace.start_attempt();
    attempt.record_started(&json!({
        "model": "gpt-test",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }],
    }));
    Ok(attempt)
}

fn output_message(id: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(id.to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

async fn replay_until_cancelled(temp: &TempDir) -> anyhow::Result<RolloutTrace> {
    let mut rollout = replay_bundle(temp.path())?;
    for _ in 0..50 {
        let inference = rollout
            .inference_calls
            .values()
            .next()
            .expect("inference should be reduced");
        if inference.execution.status == ExecutionStatus::Cancelled {
            return Ok(rollout);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        rollout = replay_bundle(temp.path())?;
    }
    Ok(rollout)
}

struct NotifyAfterEventStream {
    events: VecDeque<ResponseEvent>,
    yielded: usize,
    notify_after: usize,
    notify: Arc<Notify>,
}

impl futures::Stream for NotifyAfterEventStream {
    type Item = std::result::Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(event) = self.events.pop_front() else {
            return Poll::Pending;
        };
        self.yielded += 1;
        if self.yielded == self.notify_after {
            self.notify.notify_one();
        }
        Poll::Ready(Some(Ok(event)))
    }
}

#[test]
fn build_subagent_headers_sets_other_subagent_label() {
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::Other(
        "memory_consolidation".to_string(),
    )));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_subagent_headers_sets_internal_memory_consolidation_label() {
    let client = test_model_client(SessionSource::Internal(
        InternalSessionSource::MemoryConsolidation,
    ));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_ws_client_metadata_includes_window_lineage_and_turn_metadata() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 2,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    }));

    let thread_id = client.state.thread_id.to_string();
    let expected_window_id = format!("{thread_id}:1");
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-123"),
        expected_window_id.clone(),
        Some(parent_thread_id),
        TestForallResponsesRequestKind::Turn,
    );
    let client_metadata =
        client.build_ws_client_metadata(&responses_metadata, /*use_responses_lite*/ false);
    let parent_thread_id = parent_thread_id.to_string();
    let turn_metadata: serde_json::Value = serde_json::from_str(
        client_metadata
            .get(X_CODEX_TURN_METADATA_HEADER)
            .expect("turn metadata"),
    )
    .expect("valid turn metadata");
    for (client_key, metadata_key, expected) in [
        (
            X_CODEX_INSTALLATION_ID_HEADER,
            "installation_id",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("session_id", "session_id", thread_id.as_str()),
        ("thread_id", "thread_id", thread_id.as_str()),
        ("turn_id", "turn_id", "turn-123"),
        (
            X_CODEX_WINDOW_ID_HEADER,
            "window_id",
            expected_window_id.as_str(),
        ),
        (
            X_CODEX_PARENT_THREAD_ID_HEADER,
            "parent_thread_id",
            parent_thread_id.as_str(),
        ),
    ] {
        assert_eq!(
            client_metadata.get(client_key).map(String::as_str),
            Some(expected)
        );
        assert_eq!(turn_metadata[metadata_key].as_str(), Some(expected));
    }
    assert_eq!(
        client_metadata
            .get(X_OPENAI_SUBAGENT_HEADER)
            .map(String::as_str),
        Some("collab_spawn")
    );
}

#[tokio::test]
async fn summarize_memories_returns_empty_for_empty_input() {
    let client = test_model_client(SessionSource::Cli);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();

    let output = client
        .summarize_memories(
            Vec::new(),
            &model_info,
            /*effort*/ None,
            &session_telemetry,
        )
        .await
        .expect("empty summarize request should succeed");
    assert_eq!(output.len(), 0);
}

#[tokio::test]
async fn dropped_response_stream_traces_cancelled_partial_output() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;

    // The provider has produced one complete output item, but no terminal
    // response.completed event. The harness has enough information to keep this
    // item in history, so the trace should preserve it when the stream is
    // abandoned.
    let item = output_message("msg-1", "partial answer");
    let api_stream = futures::stream::iter([Ok(ResponseEvent::OutputItemDone(item))])
        .chain(futures::stream::pending());
    let (mut stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    let observed = stream
        .next()
        .await
        .expect("mapped stream should yield output item")?;
    assert!(matches!(observed, ResponseEvent::OutputItemDone(_)));

    // Dropping the consumer is how turn interruption/preemption stops polling
    // the provider stream. The mapper task observes that drop asynchronously
    // and records cancellation using the output items it has already seen.
    drop(stream);

    // Cancellation is recorded by the mapper task after Drop wakes it, so the
    // replay may need a short wait before the terminal event appears on disk.
    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[tokio::test]
async fn response_stream_records_last_model_feedback_ids() {
    let tags = Arc::new(Mutex::new(BTreeMap::new()));
    let _guard = tracing_subscriber::registry()
        .with(TagCollectorLayer { tags: tags.clone() })
        .set_default();

    let api_stream = futures::stream::iter([
        Ok(ResponseEvent::Created),
        Ok(ResponseEvent::Completed {
            response_id: "resp-123".to_string(),
            token_usage: None,
            end_turn: Some(true),
        }),
    ]);
    let (mut stream, _) = super::map_response_events(
        Some("req-123".to_string()),
        api_stream,
        test_session_telemetry(),
        InferenceTraceAttempt::disabled(),
        test_model_provider(),
    );

    while stream.next().await.is_some() {}

    let tags = tags.lock().unwrap().clone();
    assert_eq!(
        tags.get("last_model_request_id").map(String::as_str),
        Some("\"req-123\"")
    );
    assert_eq!(
        tags.get("last_model_response_id").map(String::as_str),
        Some("\"resp-123\"")
    );
}

#[tokio::test]
async fn bedrock_unauthorized_error_uses_provider_mapping() {
    let provider = create_model_provider(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        /*auth_manager*/ None,
    );
    let mut auth_recovery = None;
    let url = "https://bedrock-mantle.us-east-2.api.aws/openai/v1/responses";
    let error = super::handle_unauthorized(
        TransportError::Http {
            status: http::StatusCode::UNAUTHORIZED,
            url: Some(url.to_string()),
            headers: None,
            body: Some(
                "Signature expired: 20260609T133205Z is now earlier than 20260614T062525Z"
                    .to_string(),
            ),
        },
        &mut auth_recovery,
        &test_session_telemetry(),
        &provider,
    )
    .await
    .expect_err("expired Bedrock signature should fail");

    assert_eq!(
        error.to_string(),
        format!(
            "Amazon Bedrock rejected the request because its AWS signature has expired. Refresh your AWS credentials and retry. If `AWS_BEARER_TOKEN_BEDROCK` is set, update or unset it, then restart Forall, url: {url}"
        )
    );
}

#[tokio::test]
async fn dropped_backpressured_response_stream_traces_cancelled_partial_output()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;
    let backpressured_item_yielded = Arc::new(Notify::new());
    let mut events = VecDeque::new();
    for _ in 0..super::RESPONSE_STREAM_CHANNEL_CAPACITY {
        events.push_back(ResponseEvent::Created);
    }
    events.push_back(ResponseEvent::OutputItemDone(output_message(
        "msg-1",
        "partial answer",
    )));
    let api_stream = NotifyAfterEventStream {
        events,
        yielded: 0,
        notify_after: super::RESPONSE_STREAM_CHANNEL_CAPACITY + 1,
        notify: Arc::clone(&backpressured_item_yielded),
    };

    let (stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    // Fill the mapper channel with non-terminal events, then yield one output
    // item. The mapper has observed that item and is blocked trying to send it
    // downstream, so dropping the consumer covers the send-failure path rather
    // than the `consumer_dropped` select branch.
    backpressured_item_yielded.notified().await;
    drop(stream);

    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[test]
fn auth_request_telemetry_context_tracks_attached_auth_and_retry_phase() {
    let auth_context = AuthRequestTelemetryContext::new(
        Some(AuthMode::ApiKey),
        &BearerAuthProvider::for_test(Some("access-token"), Some("workspace-123")),
        PendingUnauthorizedRetry::from_recovery(UnauthorizedRecoveryExecution {
            mode: "managed",
            phase: "refresh_token",
        }),
    );

    assert_eq!(auth_context.auth_mode, Some("ApiKey"));
    assert!(auth_context.auth_header_attached);
    assert_eq!(auth_context.auth_header_name, Some("authorization"));
    assert!(auth_context.retry_after_unauthorized);
    assert_eq!(auth_context.recovery_mode, Some("managed"));
    assert_eq!(auth_context.recovery_phase, Some("refresh_token"));
}

fn model_client_with_counting_attestation(
    include_attestation: bool,
) -> (ModelClient, Arc<AtomicUsize>) {
    #[derive(Debug)]
    struct CountingAttestationProvider {
        calls: Arc<AtomicUsize>,
    }

    impl AttestationProvider for CountingAttestationProvider {
        fn header_for_request(
            &self,
            _context: AttestationContext,
        ) -> GenerateAttestationFuture<'_> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let call = calls.fetch_add(1, Ordering::Relaxed) + 1;
                Some(http::HeaderValue::from_bytes(format!("v1.header-{call}").as_bytes()).unwrap())
            })
        }
    }

    let attestation_calls = Arc::new(AtomicUsize::new(0));
    let (auth_manager, provider) = if include_attestation {
        (
            Some(AuthManager::from_auth_for_testing(
                ForallAuth::create_dummy_api_key_auth_for_testing(),
            )),
            ModelProviderInfo::create_openai_provider(None),
        )
    } else {
        (
            None,
            create_oss_provider_with_base_url("https://example.com/v1", WireApi::Responses),
        )
    };
    let model_client = ModelClient::new(
        auth_manager,
        ThreadId::new(),
        provider,
        SessionSource::Exec,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*item_ids_enabled*/ false,
        Some(Arc::new(CountingAttestationProvider {
            calls: attestation_calls.clone(),
        })),
    );
    (model_client, attestation_calls)
}

#[tokio::test]
#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
async fn websocket_handshake_includes_attestation_for_chatgpt_forall_responses() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ true);
    let responses_metadata = test_responses_metadata_for_client(
        &model_client,
        /*turn_id*/ None,
        format!("{}:0", model_client.state.thread_id),
        /*parent_thread_id*/ None,
        TestForallResponsesRequestKind::WebsocketConnection,
    );

    let headers = model_client
        .build_websocket_headers(&responses_metadata)
        .await;

    assert_eq!(
        headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("v1.header-1"),
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn non_chatgpt_forall_endpoints_omit_attestation_generation() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*include_attestation*/ false);
    let mut response_headers = http::HeaderMap::new();

    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        response_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut compaction_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        compaction_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut realtime_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        realtime_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }

    assert_eq!(
        response_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        compaction_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        realtime_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}

// ---------------------------------------------------------------------------
// Messages API request building
// ---------------------------------------------------------------------------

mod messages_request {
    use super::test_model_client;
    use super::test_model_info;
    use crate::client::MESSAGES_DEFAULT_MAX_OUTPUT_TOKENS;
    use crate::client::MESSAGES_PLACEHOLDER_USER_TEXT;
    use crate::client::apply_history_cache_breakpoint;
    use crate::client::build_messages_history;
    use crate::client::thinking_budget_tokens;
    use crate::client_common::Prompt;
    use base64::Engine as _;
    use forall_api::CacheControl;
    use forall_api::MessagesContentBlock;
    use forall_api::MessagesImageSource;
    use forall_api::MessagesMessage;
    use forall_api::MessagesRole;
    use forall_api::MessagesSystemBlock;
    use forall_api::MessagesToolChoice;
    use forall_api::MessagesToolResultContent;
    use forall_api::ThinkingConfig;
    use forall_protocol::models::BaseInstructions;
    use forall_protocol::models::ContentItem;
    use forall_protocol::models::FunctionCallOutputContentItem;
    use forall_protocol::models::FunctionCallOutputPayload;
    use forall_protocol::models::ReasoningItemContent;
    use forall_protocol::models::ResponseItem;
    use forall_protocol::openai_models::ModelInfo;
    use forall_protocol::openai_models::ReasoningEffort;
    use forall_protocol::protocol::SessionSource;
    use forall_tools::JsonSchema;
    use forall_tools::ResponsesApiTool;
    use forall_tools::ToolSpec;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn user_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn assistant_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn envelope(value: serde_json::Value) -> String {
        base64::engine::general_purpose::STANDARD.encode(value.to_string())
    }

    fn reasoning_item(encrypted_content: Option<String>) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: "visible thinking".to_string(),
            }]),
            encrypted_content,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn text_blocks(message: &MessagesMessage) -> Vec<&str> {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                MessagesContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn history_translates_text_and_image_content() {
        let history = build_messages_history(&[
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "look at this".to_string(),
                    },
                    ContentItem::InputImage {
                        image_url: "data:image/png;base64,aGVsbG8=".to_string(),
                        detail: None,
                    },
                    ContentItem::InputImage {
                        image_url: "https://example.com/cat.png".to_string(),
                        detail: None,
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            assistant_message("a cat"),
        ]);

        assert_eq!(
            history,
            vec![
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![
                        MessagesContentBlock::Text {
                            text: "look at this".to_string(),
                            cache_control: None,
                        },
                        MessagesContentBlock::Image {
                            source: MessagesImageSource::Base64 {
                                media_type: "image/png".to_string(),
                                data: "aGVsbG8=".to_string(),
                            },
                            cache_control: None,
                        },
                        MessagesContentBlock::Image {
                            source: MessagesImageSource::Url {
                                url: "https://example.com/cat.png".to_string(),
                            },
                            cache_control: None,
                        },
                    ],
                },
                MessagesMessage {
                    role: MessagesRole::Assistant,
                    content: vec![MessagesContentBlock::Text {
                        text: "a cat".to_string(),
                        cache_control: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn history_round_trips_thinking_envelope_signature() {
        let history = build_messages_history(&[
            user_message("think"),
            reasoning_item(Some(envelope(json!({
                "thinking": "visible thinking",
                "signature": "sig-abc",
            })))),
        ]);

        assert_eq!(history.len(), 2);
        assert_eq!(
            history[1],
            MessagesMessage {
                role: MessagesRole::Assistant,
                content: vec![MessagesContentBlock::Thinking {
                    thinking: "visible thinking".to_string(),
                    signature: "sig-abc".to_string(),
                }],
            }
        );
    }

    #[test]
    fn history_round_trips_redacted_thinking_envelope() {
        let history = build_messages_history(&[
            user_message("think"),
            reasoning_item(Some(envelope(json!({ "redacted": "opaque-bytes" })))),
        ]);

        assert_eq!(
            history[1].content,
            vec![MessagesContentBlock::RedactedThinking {
                data: "opaque-bytes".to_string(),
            }]
        );
    }

    #[test]
    fn history_skips_reasoning_without_decodable_envelope() {
        // No envelope at all, non-base64 junk, and a foreign (OpenAI) opaque
        // payload must all be dropped: Anthropic hard-rejects thinking blocks
        // whose signature does not match.
        let history = build_messages_history(&[
            user_message("hi"),
            reasoning_item(None),
            reasoning_item(Some("!!!not-base64!!!".to_string())),
            reasoning_item(Some(envelope(json!({ "opaque": "gAAAA..." })))),
            assistant_message("answer"),
        ]);

        assert_eq!(history.len(), 2);
        assert_eq!(history[1].content.len(), 1);
        assert_eq!(text_blocks(&history[1]), vec!["answer"]);
    }

    #[test]
    fn history_maps_function_calls_and_outputs() {
        let mut output_with_items = FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::InputText {
                text: "file list".to_string(),
            },
            FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "opaque".to_string(),
            },
        ]);
        output_with_items.success = Some(false);
        let history = build_messages_history(&[
            user_message("run it"),
            ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{\"command\":[\"ls\"]}".to_string(),
                call_id: "toolu_1".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: "toolu_1".to_string(),
                output: output_with_items,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCall {
                id: None,
                status: None,
                call_id: "toolu_2".to_string(),
                name: "apply_patch".to_string(),
                input: "not json".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "toolu_2".to_string(),
                name: None,
                output: FunctionCallOutputPayload::from_text("ok".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ]);

        assert_eq!(
            history,
            vec![
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![MessagesContentBlock::Text {
                        text: "run it".to_string(),
                        cache_control: None,
                    }],
                },
                MessagesMessage {
                    role: MessagesRole::Assistant,
                    content: vec![MessagesContentBlock::ToolUse {
                        id: "toolu_1".to_string(),
                        name: "shell".to_string(),
                        input: json!({"command": ["ls"]}),
                        cache_control: None,
                    }],
                },
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![MessagesContentBlock::ToolResult {
                        tool_use_id: "toolu_1".to_string(),
                        content: vec![
                            MessagesToolResultContent::Text {
                                text: "file list".to_string(),
                            },
                            MessagesToolResultContent::Text {
                                text: "[encrypted content omitted]".to_string(),
                            },
                        ],
                        is_error: Some(true),
                        cache_control: None,
                    }],
                },
                MessagesMessage {
                    role: MessagesRole::Assistant,
                    content: vec![MessagesContentBlock::ToolUse {
                        id: "toolu_2".to_string(),
                        name: "apply_patch".to_string(),
                        // Unparseable arguments are preserved under `_raw`.
                        input: json!({"_raw": "not json"}),
                        cache_control: None,
                    }],
                },
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![MessagesContentBlock::ToolResult {
                        tool_use_id: "toolu_2".to_string(),
                        content: vec![MessagesToolResultContent::Text {
                            text: "ok".to_string(),
                        }],
                        is_error: None,
                        cache_control: None,
                    }],
                },
            ]
        );
    }

    #[test]
    fn history_merges_consecutive_same_role_messages() {
        let history = build_messages_history(&[
            user_message("one"),
            user_message("two"),
            assistant_message("three"),
            reasoning_item(Some(envelope(json!({
                "thinking": "t",
                "signature": "s",
            })))),
        ]);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, MessagesRole::User);
        assert_eq!(text_blocks(&history[0]), vec!["one", "two"]);
        assert_eq!(history[1].role, MessagesRole::Assistant);
        assert_eq!(history[1].content.len(), 2);
    }

    #[test]
    fn history_inserts_leading_user_placeholder_when_needed() {
        // Leading assistant turn.
        let history = build_messages_history(&[assistant_message("hello")]);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, MessagesRole::User);
        assert_eq!(
            text_blocks(&history[0]),
            vec![MESSAGES_PLACEHOLDER_USER_TEXT]
        );
        assert_eq!(history[1].role, MessagesRole::Assistant);

        // Empty translation result.
        let history = build_messages_history(&[]);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, MessagesRole::User);
        assert_eq!(
            text_blocks(&history[0]),
            vec![MESSAGES_PLACEHOLDER_USER_TEXT]
        );
    }

    #[test]
    fn history_skips_unmappable_items() {
        let history = build_messages_history(&[
            user_message("hi"),
            ResponseItem::WebSearchCall {
                id: None,
                status: Some("completed".to_string()),
                action: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::ImageGenerationCall {
                id: None,
                status: "completed".to_string(),
                revised_prompt: None,
                result: "img".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::Compaction {
                id: None,
                encrypted_content: "opaque".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CompactionTrigger {},
            ResponseItem::Other,
            assistant_message("done"),
        ]);

        assert_eq!(history.len(), 2);
        assert_eq!(text_blocks(&history[0]), vec!["hi"]);
        assert_eq!(text_blocks(&history[1]), vec!["done"]);
    }

    #[test]
    fn cache_breakpoint_lands_on_second_most_recent_user_message() {
        let mut history = build_messages_history(&[
            user_message("turn one"),
            assistant_message("answer one"),
            user_message("turn two"),
            assistant_message("answer two"),
            user_message("turn three"),
        ]);
        apply_history_cache_breakpoint(&mut history);

        let cache_controls: Vec<Option<CacheControl>> = history
            .iter()
            .map(|message| match message.content.last() {
                Some(MessagesContentBlock::Text { cache_control, .. }) => *cache_control,
                _ => None,
            })
            .collect();
        assert_eq!(
            cache_controls,
            vec![
                None,
                None,
                // "turn two" is the second-most-recent user message: the
                // shared prefix through the previous turn gets cached.
                Some(CacheControl::Ephemeral),
                None,
                None,
            ]
        );
    }

    #[test]
    fn cache_breakpoint_skipped_with_fewer_than_two_user_messages() {
        let mut history = build_messages_history(&[user_message("only turn")]);
        apply_history_cache_breakpoint(&mut history);
        assert_eq!(
            history[0].content,
            vec![MessagesContentBlock::Text {
                text: "only turn".to_string(),
                cache_control: None,
            }]
        );
    }

    #[test]
    fn thinking_budget_maps_effort_levels() {
        assert_eq!(thinking_budget_tokens(&ReasoningEffort::None), None);
        assert_eq!(
            thinking_budget_tokens(&ReasoningEffort::Minimal),
            Some(4_000)
        );
        assert_eq!(thinking_budget_tokens(&ReasoningEffort::Low), Some(4_000));
        assert_eq!(
            thinking_budget_tokens(&ReasoningEffort::Medium),
            Some(10_000)
        );
        assert_eq!(
            thinking_budget_tokens(&ReasoningEffort::Custom("dynamic".to_string())),
            Some(10_000)
        );
        assert_eq!(thinking_budget_tokens(&ReasoningEffort::High), Some(24_000));
        assert_eq!(
            thinking_budget_tokens(&ReasoningEffort::XHigh),
            Some(24_000)
        );
    }

    fn function_tool(name: &str) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: name.to_string(),
            description: format!("{name} tool"),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        })
    }

    fn reasoning_model_info() -> ModelInfo {
        let mut model_info = test_model_info();
        model_info.supports_reasoning_summaries = true;
        model_info.default_reasoning_level = Some(ReasoningEffort::Medium);
        model_info
    }

    fn build_request(
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffort>,
    ) -> forall_api::MessagesApiRequest {
        test_model_client(SessionSource::Cli)
            .build_messages_request(prompt, model_info, effort)
            .expect("build messages request")
    }

    #[test]
    fn request_places_system_and_tool_cache_breakpoints() {
        let prompt = Prompt {
            base_instructions: BaseInstructions {
                text: "You are Forall.".to_string(),
            },
            input: vec![user_message("hello")],
            tools: vec![function_tool("first"), function_tool("second")],
            parallel_tool_calls: false,
            ..Prompt::default()
        };

        let request = build_request(&prompt, &test_model_info(), /*effort*/ None);

        assert_eq!(
            request.system,
            Some(vec![MessagesSystemBlock::Text {
                text: "You are Forall.".to_string(),
                cache_control: Some(CacheControl::Ephemeral),
            }])
        );
        assert_eq!(request.tools.len(), 2);
        assert_eq!(request.tools[0].cache_control, None);
        assert_eq!(
            request.tools[1].cache_control,
            Some(CacheControl::Ephemeral)
        );
        assert_eq!(
            request.tool_choice,
            Some(MessagesToolChoice::Auto {
                disable_parallel_tool_use: Some(true),
            })
        );
        assert_eq!(request.max_tokens, MESSAGES_DEFAULT_MAX_OUTPUT_TOKENS);
        assert!(request.stream);
        assert_eq!(request.metadata, None);
    }

    #[test]
    fn request_allows_parallel_tool_use_when_prompt_permits() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            tools: vec![function_tool("only")],
            parallel_tool_calls: true,
            ..Prompt::default()
        };

        let request = build_request(&prompt, &test_model_info(), /*effort*/ None);
        assert_eq!(
            request.tool_choice,
            Some(MessagesToolChoice::Auto {
                disable_parallel_tool_use: None,
            })
        );
    }

    #[test]
    fn request_omits_tool_choice_without_tools() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        let request = build_request(&prompt, &test_model_info(), /*effort*/ None);
        assert!(request.tools.is_empty());
        assert_eq!(request.tool_choice, None);
    }

    #[test]
    fn request_enables_thinking_from_resolved_effort() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        // Explicit effort wins.
        let request = build_request(
            &prompt,
            &reasoning_model_info(),
            Some(ReasoningEffort::High),
        );
        assert_eq!(
            request.thinking,
            Some(ThinkingConfig::Enabled {
                budget_tokens: 24_000,
            })
        );

        // No explicit effort falls back to the model default (medium).
        let request = build_request(&prompt, &reasoning_model_info(), /*effort*/ None);
        assert_eq!(
            request.thinking,
            Some(ThinkingConfig::Enabled {
                budget_tokens: 10_000,
            })
        );

        // Models without reasoning summaries never get a thinking config.
        let request = build_request(&prompt, &test_model_info(), Some(ReasoningEffort::High));
        assert_eq!(request.thinking, None);
    }
}

mod gemini_request {
    use super::test_model_client;
    use super::test_model_info;
    use crate::client::GEMINI_DEFAULT_MAX_OUTPUT_TOKENS;
    use crate::client::GEMINI_PLACEHOLDER_USER_TEXT;
    use crate::client::build_gemini_contents;
    use crate::client::gemini_thinking_budget;
    use crate::client_common::Prompt;
    use base64::Engine as _;
    use forall_api::GeminiBlob;
    use forall_api::GeminiContent;
    use forall_api::GeminiFunctionCall;
    use forall_api::GeminiFunctionCallingMode;
    use forall_api::GeminiPart;
    use forall_api::GeminiRole;
    use forall_api::GeminiThinkingConfig;
    use forall_protocol::models::BaseInstructions;
    use forall_protocol::models::ContentItem;
    use forall_protocol::models::FunctionCallOutputBody;
    use forall_protocol::models::FunctionCallOutputContentItem;
    use forall_protocol::models::FunctionCallOutputPayload;
    use forall_protocol::models::ReasoningItemContent;
    use forall_protocol::models::ResponseItem;
    use forall_protocol::openai_models::ModelInfo;
    use forall_protocol::openai_models::ReasoningEffort;
    use forall_protocol::protocol::SessionSource;
    use forall_tools::JsonSchema;
    use forall_tools::ResponsesApiTool;
    use forall_tools::ToolSpec;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn user_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn assistant_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call(name: &str, call_id: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: arguments.to_string(),
            call_id: call_id.to_string(),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call_output(call_id: &str, output: &str, success: Option<bool>) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(output.to_string()),
                success,
            },
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn envelope(value: serde_json::Value) -> String {
        base64::engine::general_purpose::STANDARD.encode(value.to_string())
    }

    fn reasoning_item(encrypted_content: Option<String>) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: "visible thinking".to_string(),
            }]),
            encrypted_content,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[test]
    fn history_translates_text_and_inline_image_content() {
        let contents = build_gemini_contents(&[
            ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![
                    ContentItem::InputText {
                        text: "look at this".to_string(),
                    },
                    ContentItem::InputImage {
                        image_url: "data:image/png;base64,aGVsbG8=".to_string(),
                        detail: None,
                    },
                    // Gemini cannot fetch arbitrary URLs: fileData needs a Files
                    // API URI, so this is dropped rather than mis-sent.
                    ContentItem::InputImage {
                        image_url: "https://example.com/cat.png".to_string(),
                        detail: None,
                    },
                ],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            },
            assistant_message("a cat"),
        ]);

        assert_eq!(
            contents,
            vec![
                GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![
                        GeminiPart::Text {
                            text: "look at this".to_string(),
                        },
                        GeminiPart::InlineData {
                            inline_data: GeminiBlob {
                                mime_type: "image/png".to_string(),
                                data: "aGVsbG8=".to_string(),
                            },
                        },
                    ],
                },
                GeminiContent {
                    role: GeminiRole::Model,
                    parts: vec![GeminiPart::Text {
                        text: "a cat".to_string(),
                    }],
                },
            ]
        );
    }

    #[test]
    fn history_translates_function_call_and_resolves_its_output_name() {
        let contents = build_gemini_contents(&[
            user_message("list files"),
            function_call("shell", "call-1", r#"{"command":["ls"]}"#),
            function_call_output("call-1", "a.txt", /*success*/ None),
        ]);

        assert_eq!(
            contents,
            vec![
                GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiPart::Text {
                        text: "list files".to_string(),
                    }],
                },
                GeminiContent {
                    role: GeminiRole::Model,
                    parts: vec![GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: "shell".to_string(),
                            args: json!({"command": ["ls"]}),
                        },
                    }],
                },
                GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiPart::FunctionResponse {
                        function_response: forall_api::GeminiFunctionResponse {
                            name: "shell".to_string(),
                            response: json!({"output": "a.txt"}),
                        },
                    }],
                },
            ]
        );
    }

    #[test]
    fn history_marks_failed_tool_output_as_an_error_response() {
        let contents = build_gemini_contents(&[
            user_message("run it"),
            function_call("shell", "call-1", "{}"),
            function_call_output("call-1", "boom", Some(false)),
        ]);

        assert_eq!(
            contents[2].parts,
            vec![GeminiPart::FunctionResponse {
                function_response: forall_api::GeminiFunctionResponse {
                    name: "shell".to_string(),
                    response: json!({"error": "boom"}),
                },
            }]
        );
    }

    /// Gemini matches results to calls by function name, so an output whose call
    /// is not in history has no name to carry and must be dropped rather than
    /// attributed to the wrong tool.
    #[test]
    fn history_skips_tool_output_with_no_matching_call() {
        let contents = build_gemini_contents(&[
            user_message("hello"),
            function_call_output("orphan", "result", /*success*/ None),
        ]);

        assert_eq!(
            contents,
            vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiPart::Text {
                    text: "hello".to_string(),
                }],
            }]
        );
    }

    #[test]
    fn history_translates_custom_tool_call_into_the_freeform_input_property() {
        let contents = build_gemini_contents(&[
            user_message("patch it"),
            ResponseItem::CustomToolCall {
                id: None,
                status: None,
                call_id: "call-2".to_string(),
                name: "apply_patch".to_string(),
                input: "*** Begin Patch".to_string(),
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CustomToolCallOutput {
                id: None,
                call_id: "call-2".to_string(),
                name: Some("apply_patch".to_string()),
                output: FunctionCallOutputPayload::from_text("done".to_string()),
                internal_chat_message_metadata_passthrough: None,
            },
        ]);

        assert_eq!(
            contents[1].parts,
            vec![GeminiPart::FunctionCall {
                function_call: GeminiFunctionCall {
                    name: "apply_patch".to_string(),
                    args: json!({"input": "*** Begin Patch"}),
                },
            }]
        );
        assert_eq!(
            contents[2].parts,
            vec![GeminiPart::FunctionResponse {
                function_response: forall_api::GeminiFunctionResponse {
                    name: "apply_patch".to_string(),
                    response: json!({"output": "done"}),
                },
            }]
        );
    }

    #[test]
    fn history_preserves_unparseable_tool_arguments_under_a_raw_key() {
        let contents = build_gemini_contents(&[
            user_message("hi"),
            function_call("shell", "call-1", "not json"),
        ]);

        assert_eq!(
            contents[1].parts,
            vec![GeminiPart::FunctionCall {
                function_call: GeminiFunctionCall {
                    name: "shell".to_string(),
                    args: json!({"_raw": "not json"}),
                },
            }]
        );
    }

    /// `functionCall.args` must be a JSON object, so a valid-but-scalar payload
    /// is still wrapped rather than sent as-is.
    #[test]
    fn history_wraps_non_object_tool_arguments() {
        let contents =
            build_gemini_contents(&[user_message("hi"), function_call("shell", "c", "[1,2]")]);

        assert_eq!(
            contents[1].parts,
            vec![GeminiPart::FunctionCall {
                function_call: GeminiFunctionCall {
                    name: "shell".to_string(),
                    args: json!({"_raw": "[1,2]"}),
                },
            }]
        );
    }

    #[test]
    fn history_round_trips_a_signed_thought_envelope() {
        let contents = build_gemini_contents(&[
            user_message("think"),
            reasoning_item(Some(envelope(
                json!({"text": "pondering", "thoughtSignature": "sig-1"}),
            ))),
        ]);

        assert_eq!(
            contents[1],
            GeminiContent {
                role: GeminiRole::Model,
                parts: vec![GeminiPart::Thought {
                    text: "pondering".to_string(),
                    thought: true,
                    thought_signature: Some("sig-1".to_string()),
                }],
            }
        );
    }

    /// An unsigned thought, and reasoning carried over from another wire
    /// protocol, are both dropped: replaying them would either lose the
    /// signature pairing or send a foreign blob.
    #[test]
    fn history_skips_reasoning_without_a_decodable_gemini_envelope() {
        for encrypted_content in [
            None,
            Some("not base64!".to_string()),
            // The Anthropic thinking envelope must not cross wire protocols.
            Some(envelope(
                json!({"thinking": "anthropic", "signature": "sig"}),
            )),
            Some(envelope(json!({"text": "no signature"}))),
        ] {
            let contents =
                build_gemini_contents(&[user_message("hi"), reasoning_item(encrypted_content)]);

            assert_eq!(
                contents,
                vec![GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiPart::Text {
                        text: "hi".to_string(),
                    }],
                }]
            );
        }
    }

    #[test]
    fn history_merges_consecutive_same_role_items() {
        let contents = build_gemini_contents(&[
            user_message("first"),
            user_message("second"),
            assistant_message("reply"),
            function_call("shell", "c", "{}"),
        ]);

        assert_eq!(contents.len(), 2);
        assert_eq!(
            contents[0].parts,
            vec![
                GeminiPart::Text {
                    text: "first".to_string(),
                },
                GeminiPart::Text {
                    text: "second".to_string(),
                },
            ]
        );
        assert_eq!(contents[1].role, GeminiRole::Model);
        assert_eq!(contents[1].parts.len(), 2);
    }

    /// Gemini does not require the first turn to be `user` (unlike the Messages
    /// API), so a history starting with a model turn is left alone.
    #[test]
    fn history_starting_with_a_model_turn_is_not_padded() {
        let contents = build_gemini_contents(&[assistant_message("hello")]);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, GeminiRole::Model);
    }

    #[test]
    fn empty_history_gets_a_placeholder_user_turn() {
        let contents = build_gemini_contents(&[]);

        assert_eq!(
            contents,
            vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiPart::Text {
                    text: GEMINI_PLACEHOLDER_USER_TEXT.to_string(),
                }],
            }]
        );
    }

    #[test]
    fn history_skips_items_with_no_gemini_representation() {
        let contents = build_gemini_contents(&[
            user_message("hi"),
            ResponseItem::WebSearchCall {
                id: None,
                status: None,
                action: None,
                internal_chat_message_metadata_passthrough: None,
            },
            ResponseItem::CompactionTrigger {},
            ResponseItem::Other,
        ]);

        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0].role, GeminiRole::User);
    }

    #[test]
    fn history_replaces_image_and_encrypted_tool_output_with_placeholders() {
        let contents = build_gemini_contents(&[
            user_message("hi"),
            function_call("shell", "call-1", "{}"),
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: "call-1".to_string(),
                internal_chat_message_metadata_passthrough: None,
                output: FunctionCallOutputPayload::from_content_items(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "text part".to_string(),
                    },
                    FunctionCallOutputContentItem::InputImage {
                        image_url: "data:image/png;base64,AAAA".to_string(),
                        detail: None,
                    },
                    FunctionCallOutputContentItem::EncryptedContent {
                        encrypted_content: "opaque".to_string(),
                    },
                ]),
            },
        ]);

        assert_eq!(
            contents[2].parts,
            vec![GeminiPart::FunctionResponse {
                function_response: forall_api::GeminiFunctionResponse {
                    name: "shell".to_string(),
                    response: json!({
                        "output": "text part\n[image omitted]\n[encrypted content omitted]"
                    }),
                },
            }]
        );
    }

    // -----------------------------------------------------------------------
    // Request assembly
    // -----------------------------------------------------------------------

    fn function_tool(name: &str) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: name.to_string(),
            description: format!("{name} tool"),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        })
    }

    fn reasoning_model_info() -> ModelInfo {
        let mut model_info = test_model_info();
        model_info.supports_reasoning_summaries = true;
        model_info.default_reasoning_level = Some(ReasoningEffort::Medium);
        model_info
    }

    fn build_request(
        prompt: &Prompt,
        model_info: &ModelInfo,
        effort: Option<ReasoningEffort>,
    ) -> forall_api::GeminiRequest {
        test_model_client(SessionSource::Cli)
            .build_gemini_request(prompt, model_info, effort)
            .expect("build gemini request")
    }

    #[test]
    fn request_nests_tools_under_a_single_function_declarations_entry() {
        let prompt = Prompt {
            base_instructions: BaseInstructions {
                text: "You are Forall.".to_string(),
            },
            input: vec![user_message("hello")],
            tools: vec![function_tool("first"), function_tool("second")],
            parallel_tool_calls: false,
            ..Prompt::default()
        };

        let request = build_request(&prompt, &test_model_info(), /*effort*/ None);

        assert_eq!(
            request.system_instruction,
            Some(forall_api::GeminiSystemInstruction {
                parts: vec![GeminiPart::Text {
                    text: "You are Forall.".to_string(),
                }],
            })
        );
        assert_eq!(request.tools.len(), 1);
        assert_eq!(
            request.tools[0]
                .function_declarations
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
        // Gemini has no parallel-tool-call toggle, so `parallel_tool_calls:
        // false` cannot be expressed and `AUTO` is sent regardless.
        assert_eq!(
            request
                .tool_config
                .as_ref()
                .map(|config| config.function_calling_config.mode),
            Some(GeminiFunctionCallingMode::Auto)
        );
    }

    #[test]
    fn request_omits_tools_and_tool_config_without_tools() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        let request = build_request(&prompt, &test_model_info(), /*effort*/ None);
        assert!(request.tools.is_empty());
        assert_eq!(request.tool_config, None);
    }

    #[test]
    fn request_omits_system_instruction_without_base_instructions() {
        let prompt = Prompt {
            base_instructions: BaseInstructions {
                text: String::new(),
            },
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        assert_eq!(
            build_request(&prompt, &test_model_info(), /*effort*/ None).system_instruction,
            None
        );
    }

    /// The Gemini default must not be the 8_192 non-OpenAI Responses hardcode:
    /// the thinking budget is spent out of this same allowance.
    #[test]
    fn request_uses_the_gemini_max_output_tokens_default() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        let generation_config = build_request(&prompt, &test_model_info(), /*effort*/ None)
            .generation_config
            .expect("generation config");
        assert_eq!(
            generation_config.max_output_tokens,
            Some(GEMINI_DEFAULT_MAX_OUTPUT_TOKENS)
        );
        assert_eq!(GEMINI_DEFAULT_MAX_OUTPUT_TOKENS, 32_768);
        assert_eq!(generation_config.temperature, None);
    }

    #[test]
    fn request_enables_thinking_from_resolved_effort() {
        let prompt = Prompt {
            input: vec![user_message("hello")],
            ..Prompt::default()
        };

        // Explicit effort wins, and thought summaries are requested so the
        // stream has something to render and round-trip.
        let request = build_request(
            &prompt,
            &reasoning_model_info(),
            Some(ReasoningEffort::High),
        );
        assert_eq!(
            request
                .generation_config
                .as_ref()
                .and_then(|config| config.thinking_config),
            Some(GeminiThinkingConfig {
                thinking_budget: Some(24_000),
                include_thoughts: Some(true),
            })
        );

        // No explicit effort falls back to the model default (medium).
        let request = build_request(&prompt, &reasoning_model_info(), /*effort*/ None);
        assert_eq!(
            request
                .generation_config
                .as_ref()
                .and_then(|config| config.thinking_config),
            Some(GeminiThinkingConfig {
                thinking_budget: Some(10_000),
                include_thoughts: Some(true),
            })
        );

        // Models without reasoning summaries never get a thinking config.
        let request = build_request(&prompt, &test_model_info(), Some(ReasoningEffort::High));
        assert_eq!(
            request
                .generation_config
                .as_ref()
                .and_then(|config| config.thinking_config),
            None
        );
    }

    #[test]
    fn thinking_budget_maps_effort_levels() {
        assert_eq!(gemini_thinking_budget(&ReasoningEffort::None), None);
        assert_eq!(
            gemini_thinking_budget(&ReasoningEffort::Minimal),
            Some(4_000)
        );
        assert_eq!(gemini_thinking_budget(&ReasoningEffort::Low), Some(4_000));
        assert_eq!(
            gemini_thinking_budget(&ReasoningEffort::Medium),
            Some(10_000)
        );
        assert_eq!(
            gemini_thinking_budget(&ReasoningEffort::Custom("dynamic".to_string())),
            Some(10_000)
        );
        assert_eq!(gemini_thinking_budget(&ReasoningEffort::High), Some(24_000));
        assert_eq!(
            gemini_thinking_budget(&ReasoningEffort::XHigh),
            Some(24_000)
        );
    }
}
