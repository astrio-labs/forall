use crate::client_config::AnalyticsEventsClientConfig;
use crate::events::AppServerRpcTransport;
use crate::events::GuardianReviewAnalyticsResult;
use crate::events::GuardianReviewTrackContext;
use crate::events::TrackEventRequest;
use crate::events::TrackEventsRequest;
use crate::events::current_runtime_metadata;
use crate::facts::AnalyticsFact;
use crate::facts::AnalyticsJsonRpcError;
use crate::facts::AppInvocation;
use crate::facts::AppMentionedInput;
use crate::facts::AppUsedInput;
use crate::facts::CustomAnalyticsFact;
use crate::facts::ExternalAgentConfigImportCompletedInput;
use crate::facts::ExternalAgentConfigImportFailureInput;
use crate::facts::ForallGoalEvent;
use crate::facts::HookRunFact;
use crate::facts::HookRunInput;
use crate::facts::PluginInstallFailedInput;
use crate::facts::PluginState;
use crate::facts::PluginStateChangedInput;
use crate::facts::SkillInvocation;
use crate::facts::SkillInvokedInput;
use crate::facts::SubAgentThreadStartedInput;
use crate::facts::TrackEventsContext;
use crate::facts::TurnForallErrorFact;
use crate::facts::TurnProfileFact;
use crate::facts::TurnResolvedConfigFact;
use crate::facts::TurnTokenUsageFact;
use crate::reducer::AnalyticsReducer;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::ClientResponsePayload;
use forall_app_server_protocol::InitializeParams;
use forall_app_server_protocol::JSONRPCErrorError;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::ServerNotification;
use forall_app_server_protocol::ServerRequest;
use forall_app_server_protocol::ServerResponse;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_login::default_client::create_client;
use forall_plugin::PluginTelemetryMetadata;
use forall_protocol::request_permissions::RequestPermissionsResponse;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

const ANALYTICS_EVENTS_QUEUE_SIZE: usize = 256;
const ANALYTICS_EVENTS_TIMEOUT: Duration = Duration::from_secs(10);
const ANALYTICS_EVENT_DEDUPE_MAX_KEYS: usize = 4096;

#[derive(Clone)]
pub(crate) struct AnalyticsEventsQueue {
    pub(crate) sender: mpsc::Sender<AnalyticsFact>,
    pub(crate) app_used_emitted_keys: Arc<Mutex<HashSet<(String, String)>>>,
    pub(crate) plugin_used_emitted_keys: Arc<Mutex<HashSet<(String, String)>>>,
}

#[derive(Clone)]
pub struct AnalyticsEventsClient {
    queue: Option<AnalyticsEventsQueue>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AnalyticsEventsDestination {
    Http {
        url: String,
    },
    #[cfg(debug_assertions)]
    CaptureFile {
        path: PathBuf,
    },
}

impl AnalyticsEventsDestination {
    fn from_endpoint_and_capture_file(endpoint: String, capture_file: Option<PathBuf>) -> Self {
        #[cfg(debug_assertions)]
        if let Some(path) = capture_file {
            if let Err(err) = crate::analytics_capture::initialize(&path) {
                tracing::error!(
                    path = %path.display(),
                    "failed to initialize analytics event capture; network delivery remains disabled: {err}"
                );
            }
            tracing::warn!(
                path = %path.display(),
                "analytics event capture enabled; network delivery is disabled"
            );
            return Self::CaptureFile { path };
        }

        #[cfg(not(debug_assertions))]
        let _ = capture_file;

        Self::Http { url: endpoint }
    }
}

fn analytics_capture_file_from_env() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os(crate::analytics_capture::ANALYTICS_EVENTS_CAPTURE_FILE_ENV_VAR)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    #[cfg(not(debug_assertions))]
    None
}

fn resolve_analytics_destination(
    config: &AnalyticsEventsClientConfig,
) -> Option<AnalyticsEventsDestination> {
    let capture_file = analytics_capture_file_from_env();
    if let Some(endpoint) = config.resolve_endpoint() {
        return Some(AnalyticsEventsDestination::from_endpoint_and_capture_file(
            endpoint,
            capture_file,
        ));
    }

    #[cfg(debug_assertions)]
    if let Some(path) = capture_file {
        if let Err(err) = crate::analytics_capture::initialize(&path) {
            tracing::error!(
                path = %path.display(),
                "failed to initialize analytics event capture: {err}"
            );
            return None;
        }
        tracing::warn!(
            path = %path.display(),
            "analytics event capture enabled; network delivery is disabled"
        );
        return Some(AnalyticsEventsDestination::CaptureFile { path });
    }

    None
}

impl AnalyticsEventsQueue {
    fn new(
        auth_manager: Arc<AuthManager>,
        destination: AnalyticsEventsDestination,
        api_key_env: String,
    ) -> Self {
        let (sender, mut receiver) = mpsc::channel(ANALYTICS_EVENTS_QUEUE_SIZE);
        tokio::spawn(async move {
            let mut reducer = AnalyticsReducer::default();
            while let Some(input) = receiver.recv().await {
                let mut events = Vec::new();
                reducer.ingest(input, &mut events).await;
                send_track_events(&auth_manager, &destination, &api_key_env, events).await;
            }
        });
        Self {
            sender,
            app_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
            plugin_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn try_send(&self, input: AnalyticsFact) {
        if self.sender.try_send(input).is_err() {
            //TODO: add a metric for this
            tracing::warn!("dropping analytics events: queue is full");
        }
    }

    pub(crate) fn should_enqueue_app_used(
        &self,
        tracking: &TrackEventsContext,
        app: &AppInvocation,
    ) -> bool {
        let Some(connector_id) = app.connector_id.as_ref() else {
            return true;
        };
        let mut emitted = self
            .app_used_emitted_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if emitted.len() >= ANALYTICS_EVENT_DEDUPE_MAX_KEYS {
            emitted.clear();
        }
        emitted.insert((tracking.turn_id.clone(), connector_id.clone()))
    }

    pub(crate) fn should_enqueue_plugin_used(
        &self,
        tracking: &TrackEventsContext,
        plugin: &PluginTelemetryMetadata,
    ) -> bool {
        let mut emitted = self
            .plugin_used_emitted_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if emitted.len() >= ANALYTICS_EVENT_DEDUPE_MAX_KEYS {
            emitted.clear();
        }
        emitted.insert((tracking.turn_id.clone(), plugin.plugin_id.as_key()))
    }
}

impl AnalyticsEventsClient {
    pub fn new(auth_manager: Arc<AuthManager>, config: AnalyticsEventsClientConfig) -> Self {
        if config.analytics_enabled == Some(false) {
            return Self { queue: None };
        }

        let Some(destination) = resolve_analytics_destination(&config) else {
            tracing::debug!(
                "product analytics enabled but no destination resolved; events will not be sent"
            );
            return Self { queue: None };
        };

        let api_key_env = config.api_key_env_var().to_string();
        Self {
            queue: Some(AnalyticsEventsQueue::new(
                Arc::clone(&auth_manager),
                destination,
                api_key_env,
            )),
        }
    }

    pub fn disabled() -> Self {
        Self { queue: None }
    }

    pub fn track_skill_invocations(
        &self,
        tracking: TrackEventsContext,
        invocations: Vec<SkillInvocation>,
    ) {
        if invocations.is_empty() {
            return;
        }
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::SkillInvoked(
            SkillInvokedInput {
                tracking,
                invocations,
            },
        )));
    }

    pub fn track_initialize(
        &self,
        connection_id: u64,
        params: InitializeParams,
        product_client_id: String,
        rpc_transport: AppServerRpcTransport,
    ) {
        self.record_fact(AnalyticsFact::Initialize {
            connection_id,
            params,
            product_client_id,
            runtime: current_runtime_metadata(),
            rpc_transport,
        });
    }

    pub fn track_subagent_thread_started(&self, input: SubAgentThreadStartedInput) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::SubAgentThreadStarted(input),
        ));
    }

    pub fn track_guardian_review(
        &self,
        tracking: &GuardianReviewTrackContext,
        result: GuardianReviewAnalyticsResult,
        completed_at_ms: u64,
    ) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::GuardianReview(
            Box::new(tracking.event_params(result, completed_at_ms)),
        )));
    }

    pub fn track_app_mentioned(&self, tracking: TrackEventsContext, mentions: Vec<AppInvocation>) {
        if mentions.is_empty() {
            return;
        }
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::AppMentioned(
            AppMentionedInput { tracking, mentions },
        )));
    }

    pub fn track_request(
        &self,
        connection_id: u64,
        request_id: RequestId,
        request: &ClientRequest,
    ) {
        if !matches!(
            request,
            ClientRequest::TurnStart { .. } | ClientRequest::TurnSteer { .. }
        ) {
            return;
        }
        self.record_fact(AnalyticsFact::ClientRequest {
            connection_id,
            request_id,
            request: Box::new(request.clone()),
        });
    }

    pub fn track_app_used(&self, tracking: TrackEventsContext, app: AppInvocation) {
        let Some(queue) = self.queue.as_ref() else {
            return;
        };
        if !queue.should_enqueue_app_used(&tracking, &app) {
            return;
        }
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::AppUsed(
            AppUsedInput { tracking, app },
        )));
    }

    pub fn track_hook_run(&self, tracking: TrackEventsContext, hook: HookRunFact) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::HookRun(
            HookRunInput { tracking, hook },
        )));
    }

    pub fn track_plugin_used(&self, tracking: TrackEventsContext, plugin: PluginTelemetryMetadata) {
        let Some(queue) = self.queue.as_ref() else {
            return;
        };
        if !queue.should_enqueue_plugin_used(&tracking, &plugin) {
            return;
        }
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::PluginUsed(
            crate::facts::PluginUsedInput { tracking, plugin },
        )));
    }

    pub fn track_compaction(&self, event: crate::facts::ForallCompactionEvent) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::Compaction(
            Box::new(event),
        )));
    }

    pub fn track_goal_event(&self, event: ForallGoalEvent) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::Goal(Box::new(
            event,
        ))));
    }

    pub fn track_turn_resolved_config(&self, fact: TurnResolvedConfigFact) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::TurnResolvedConfig(Box::new(fact)),
        ));
    }

    pub fn track_turn_token_usage(&self, fact: TurnTokenUsageFact) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::TurnTokenUsage(
            Box::new(fact),
        )));
    }

    pub fn track_turn_profile(&self, fact: TurnProfileFact) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::TurnProfile(
            Box::new(fact),
        )));
    }

    pub fn track_turn_forall_error(&self, fact: TurnForallErrorFact) {
        self.record_fact(AnalyticsFact::Custom(CustomAnalyticsFact::TurnForallError(
            Box::new(fact),
        )));
    }

    pub fn track_plugin_installed(&self, plugin: PluginTelemetryMetadata) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::PluginStateChanged(PluginStateChangedInput {
                plugin,
                state: PluginState::Installed,
            }),
        ));
    }

    pub fn track_plugin_install_failed(&self, plugin: PluginTelemetryMetadata, error_type: String) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::PluginInstallFailed(PluginInstallFailedInput {
                plugin,
                error_type,
            }),
        ));
    }

    pub fn track_external_agent_config_import_completed(
        &self,
        input: ExternalAgentConfigImportCompletedInput,
    ) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::ExternalAgentConfigImportCompleted(input),
        ));
    }

    pub fn track_external_agent_config_import_failure(
        &self,
        input: ExternalAgentConfigImportFailureInput,
    ) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::ExternalAgentConfigImportFailure(input),
        ));
    }

    pub fn track_forall_check_completed(&self, input: crate::facts::ForallCheckCompletedInput) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::ForallCheckCompleted(input),
        ));
    }

    pub fn track_verified_gate_blocked(&self, input: crate::facts::VerifiedGateBlockedInput) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::VerifiedGateBlocked(input),
        ));
    }

    pub fn track_plugin_uninstalled(&self, plugin: PluginTelemetryMetadata) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::PluginStateChanged(PluginStateChangedInput {
                plugin,
                state: PluginState::Uninstalled,
            }),
        ));
    }

    pub fn track_plugin_enabled(&self, plugin: PluginTelemetryMetadata) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::PluginStateChanged(PluginStateChangedInput {
                plugin,
                state: PluginState::Enabled,
            }),
        ));
    }

    pub fn track_plugin_disabled(&self, plugin: PluginTelemetryMetadata) {
        self.record_fact(AnalyticsFact::Custom(
            CustomAnalyticsFact::PluginStateChanged(PluginStateChangedInput {
                plugin,
                state: PluginState::Disabled,
            }),
        ));
    }

    pub(crate) fn record_fact(&self, input: AnalyticsFact) {
        if let Some(queue) = self.queue.as_ref() {
            queue.try_send(input);
        }
    }

    pub fn track_response(
        &self,
        connection_id: u64,
        request_id: RequestId,
        response: ClientResponsePayload,
    ) {
        if !matches!(
            response,
            ClientResponsePayload::ThreadStart(_)
                | ClientResponsePayload::ThreadResume(_)
                | ClientResponsePayload::ThreadFork(_)
                | ClientResponsePayload::TurnStart(_)
                | ClientResponsePayload::TurnSteer(_)
        ) {
            return;
        }
        self.record_fact(AnalyticsFact::ClientResponse {
            connection_id,
            request_id,
            response: Box::new(response),
        });
    }

    pub fn track_error_response(
        &self,
        connection_id: u64,
        request_id: RequestId,
        error: JSONRPCErrorError,
        error_type: Option<AnalyticsJsonRpcError>,
    ) {
        self.record_fact(AnalyticsFact::ErrorResponse {
            connection_id,
            request_id,
            error,
            error_type,
        });
    }

    pub fn track_server_request(&self, connection_id: u64, request: ServerRequest) {
        self.record_fact(AnalyticsFact::ServerRequest {
            connection_id,
            request: Box::new(request),
        });
    }

    pub fn track_server_response(&self, completed_at_ms: u64, response: ServerResponse) {
        self.record_fact(AnalyticsFact::ServerResponse {
            completed_at_ms,
            response: Box::new(response),
        });
    }

    pub fn track_effective_permissions_approval_response(
        &self,
        completed_at_ms: u64,
        request_id: RequestId,
        response: RequestPermissionsResponse,
    ) {
        self.record_fact(AnalyticsFact::EffectivePermissionsApprovalResponse {
            completed_at_ms,
            request_id,
            response: Box::new(response),
        });
    }

    pub fn track_server_request_aborted(&self, completed_at_ms: u64, request_id: RequestId) {
        self.record_fact(AnalyticsFact::ServerRequestAborted {
            completed_at_ms,
            request_id,
        });
    }

    pub fn track_notification(&self, notification: ServerNotification) {
        if !matches!(
            notification,
            ServerNotification::TurnStarted(_)
                | ServerNotification::TurnCompleted(_)
                | ServerNotification::TurnDiffUpdated(_)
                | ServerNotification::ItemStarted(_)
                | ServerNotification::ItemCompleted(_)
                | ServerNotification::ItemGuardianApprovalReviewStarted(_)
                | ServerNotification::ItemGuardianApprovalReviewCompleted(_)
        ) {
            return;
        }
        self.record_fact(AnalyticsFact::Notification(Box::new(notification)));
    }
}

async fn send_track_events(
    auth_manager: &AuthManager,
    destination: &AnalyticsEventsDestination,
    api_key_env: &str,
    events: Vec<TrackEventRequest>,
) {
    if events.is_empty() {
        return;
    }

    let auth = auth_manager.auth().await;
    for events in track_event_request_batches(events) {
        send_track_events_request(auth.as_ref(), destination, api_key_env, events).await;
    }
}

fn track_event_request_batches(events: Vec<TrackEventRequest>) -> Vec<Vec<TrackEventRequest>> {
    let mut batches = Vec::new();
    let mut current_batch = Vec::new();

    for event in events {
        if event.should_send_in_isolated_request() {
            if !current_batch.is_empty() {
                batches.push(current_batch);
                current_batch = Vec::new();
            }
            batches.push(vec![event]);
        } else {
            current_batch.push(event);
        }
    }

    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    batches
}

async fn send_track_events_request(
    auth: Option<&ForallAuth>,
    destination: &AnalyticsEventsDestination,
    api_key_env: &str,
    events: Vec<TrackEventRequest>,
) {
    if events.is_empty() {
        return;
    }

    let payload = TrackEventsRequest { events };

    #[cfg(debug_assertions)]
    if capture_track_events_request(destination, &payload) {
        return;
    }

    let url = match destination {
        AnalyticsEventsDestination::Http { url } => url,
        #[cfg(debug_assertions)]
        AnalyticsEventsDestination::CaptureFile { .. } => return,
    };

    let mut request = create_client()
        .post(url)
        .timeout(ANALYTICS_EVENTS_TIMEOUT)
        .header("Content-Type", "application/json");

    if let Some(api_key) = crate::client_config::resolve_telemetry_api_key(api_key_env) {
        request = request.header("Authorization", format!("Bearer {api_key}"));
    } else if let Some(auth) = auth {
        request =
            request.headers(forall_model_provider::auth_provider_from_auth(auth).to_auth_headers());
    } else {
        tracing::debug!(
            api_key_env,
            "skipping analytics delivery: no telemetry API key (runtime or embedded) or auth available"
        );
        return;
    }

    let response = request.json(&payload).send().await;

    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!("events failed with status {status}: {body}");
        }
        Err(err) => {
            tracing::warn!("failed to send events request: {err}");
        }
    }
}

#[cfg(debug_assertions)]
fn capture_track_events_request(
    destination: &AnalyticsEventsDestination,
    payload: &TrackEventsRequest,
) -> bool {
    let AnalyticsEventsDestination::CaptureFile { path } = destination else {
        return false;
    };

    if let Err(err) = crate::analytics_capture::append_payload(path, payload) {
        tracing::error!(
            path = %path.display(),
            "failed to capture analytics events; network delivery remains disabled: {err}"
        );
    }
    true
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
