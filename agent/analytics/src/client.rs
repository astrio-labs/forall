use crate::events::AppServerRpcTransport;
use crate::events::GuardianReviewAnalyticsResult;
use crate::events::GuardianReviewTrackContext;
use crate::facts::AnalyticsJsonRpcError;
use crate::facts::AppInvocation;
use crate::facts::ExternalAgentConfigImportCompletedInput;
use crate::facts::ExternalAgentConfigImportFailureInput;
use crate::facts::ForallGoalEvent;
use crate::facts::HookRunFact;
use crate::facts::SkillInvocation;
use crate::facts::SubAgentThreadStartedInput;
use crate::facts::TrackEventsContext;
use crate::facts::TurnForallErrorFact;
use crate::facts::TurnProfileFact;
use crate::facts::TurnResolvedConfigFact;
use crate::facts::TurnTokenUsageFact;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::ClientResponsePayload;
use forall_app_server_protocol::InitializeParams;
use forall_app_server_protocol::JSONRPCErrorError;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::ServerNotification;
use forall_app_server_protocol::ServerRequest;
use forall_app_server_protocol::ServerResponse;
use forall_plugin::PluginTelemetryMetadata;
use forall_protocol::request_permissions::RequestPermissionsResponse;

/// Compatibility handle for runtime extensions. Forall has no analytics backend.
#[derive(Clone, Default)]
pub struct AnalyticsEventsClient;

impl AnalyticsEventsClient {
    pub fn disabled() -> Self {
        Self
    }

    pub fn track_skill_invocations(
        &self,
        _tracking: TrackEventsContext,
        _invocations: Vec<SkillInvocation>,
    ) {
    }

    pub fn track_initialize(
        &self,
        _connection_id: u64,
        _params: InitializeParams,
        _product_client_id: String,
        _rpc_transport: AppServerRpcTransport,
    ) {
    }

    pub fn track_subagent_thread_started(&self, _input: SubAgentThreadStartedInput) {}

    pub fn track_guardian_review(
        &self,
        _tracking: &GuardianReviewTrackContext,
        _result: GuardianReviewAnalyticsResult,
        _completed_at_ms: u64,
    ) {
    }

    pub fn track_app_mentioned(
        &self,
        _tracking: TrackEventsContext,
        _mentions: Vec<AppInvocation>,
    ) {
    }

    pub fn track_request(
        &self,
        _connection_id: u64,
        _request_id: RequestId,
        _request: &ClientRequest,
    ) {
    }

    pub fn track_app_used(&self, _tracking: TrackEventsContext, _app: AppInvocation) {}

    pub fn track_hook_run(&self, _tracking: TrackEventsContext, _hook: HookRunFact) {}

    pub fn track_plugin_used(
        &self,
        _tracking: TrackEventsContext,
        _plugin: PluginTelemetryMetadata,
    ) {
    }

    pub fn track_compaction(&self, _event: crate::facts::ForallCompactionEvent) {}

    pub fn track_goal_event(&self, _event: ForallGoalEvent) {}

    pub fn track_turn_resolved_config(&self, _fact: TurnResolvedConfigFact) {}

    pub fn track_turn_token_usage(&self, _fact: TurnTokenUsageFact) {}

    pub fn track_turn_profile(&self, _fact: TurnProfileFact) {}

    pub fn track_turn_forall_error(&self, _fact: TurnForallErrorFact) {}

    pub fn track_plugin_installed(&self, _plugin: PluginTelemetryMetadata) {}

    pub fn track_plugin_install_failed(
        &self,
        _plugin: PluginTelemetryMetadata,
        _error_type: String,
    ) {
    }

    pub fn track_external_agent_config_import_completed(
        &self,
        _input: ExternalAgentConfigImportCompletedInput,
    ) {
    }

    pub fn track_external_agent_config_import_failure(
        &self,
        _input: ExternalAgentConfigImportFailureInput,
    ) {
    }

    pub fn track_forall_check_completed(&self, _input: crate::facts::ForallCheckCompletedInput) {}

    pub fn track_verified_gate_blocked(&self, _input: crate::facts::VerifiedGateBlockedInput) {}

    pub fn track_plugin_uninstalled(&self, _plugin: PluginTelemetryMetadata) {}

    pub fn track_plugin_enabled(&self, _plugin: PluginTelemetryMetadata) {}

    pub fn track_plugin_disabled(&self, _plugin: PluginTelemetryMetadata) {}

    pub fn track_response(
        &self,
        _connection_id: u64,
        _request_id: RequestId,
        _response: ClientResponsePayload,
    ) {
    }

    pub fn track_error_response(
        &self,
        _connection_id: u64,
        _request_id: RequestId,
        _error: JSONRPCErrorError,
        _error_type: Option<AnalyticsJsonRpcError>,
    ) {
    }

    pub fn track_server_request(&self, _connection_id: u64, _request: ServerRequest) {}

    pub fn track_server_response(&self, _completed_at_ms: u64, _response: ServerResponse) {}

    pub fn track_effective_permissions_approval_response(
        &self,
        _completed_at_ms: u64,
        _request_id: RequestId,
        _response: RequestPermissionsResponse,
    ) {
    }

    pub fn track_server_request_aborted(&self, _completed_at_ms: u64, _request_id: RequestId) {}

    pub fn track_notification(&self, _notification: ServerNotification) {}
}
