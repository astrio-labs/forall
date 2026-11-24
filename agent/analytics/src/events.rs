use std::time::Instant;

use crate::facts::AcceptedLineFingerprint;
use crate::facts::AppInvocation;
use crate::facts::CompactionImplementation;
use crate::facts::CompactionPhase;
use crate::facts::CompactionReason;
use crate::facts::CompactionStatus;
use crate::facts::CompactionStrategy;
use crate::facts::CompactionTrigger;
use crate::facts::ForallCheckCompletedFact;
use crate::facts::ForallCompactionEvent;
use crate::facts::ForallErrKind;
use crate::facts::ForallGoalEvent;
use crate::facts::GoalEventKind;
use crate::facts::HookRunFact;
use crate::facts::InvocationType;
use crate::facts::PluginState;
use crate::facts::SubAgentThreadStartedInput;
use crate::facts::ThreadInitializationMode;
use crate::facts::TrackEventsContext;
use crate::facts::TurnStatus;
use crate::facts::TurnSteerRejectionReason;
use crate::facts::TurnSteerResult;
use crate::facts::TurnSubmissionType;
use crate::facts::VerifiedGateBlockedFact;
use crate::now_unix_millis;
use forall_app_server_protocol::CommandExecutionSource;
use forall_app_server_protocol::ForallErrorInfo;
use forall_login::default_client::originator;
use forall_plugin::PluginTelemetryMetadata;
use forall_protocol::approvals::NetworkApprovalProtocol;
use forall_protocol::models::AdditionalPermissionProfile;
use forall_protocol::models::SandboxPermissions;
use forall_protocol::protocol::GuardianAssessmentOutcome;
use forall_protocol::protocol::GuardianCommandSource;
use forall_protocol::protocol::GuardianRiskLevel;
use forall_protocol::protocol::GuardianUserAuthorization;
use forall_protocol::protocol::HookEventName;
use forall_protocol::protocol::HookRunStatus;
use forall_protocol::protocol::HookSource;
use forall_protocol::protocol::SubAgentSource;
use forall_protocol::protocol::ThreadSource;
use forall_protocol::protocol::TokenUsage;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppServerRpcTransport {
    Stdio,
    Websocket,
    InProcess,
}

#[derive(Serialize)]
pub(crate) struct TrackEventsRequest {
    pub(crate) events: Vec<TrackEventRequest>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum TrackEventRequest {
    SkillInvocation(SkillInvocationEventRequest),
    ThreadInitialized(ThreadInitializedEvent),
    GuardianReview(Box<GuardianReviewEventRequest>),
    AppMentioned(ForallAppMentionedEventRequest),
    AppUsed(ForallAppUsedEventRequest),
    HookRun(ForallHookRunEventRequest),
    Compaction(Box<ForallCompactionEventRequest>),
    Goal(Box<ForallGoalEventRequest>),
    TurnEvent(Box<ForallTurnEventRequest>),
    TurnSteer(ForallTurnSteerEventRequest),
    CommandExecution(ForallCommandExecutionEventRequest),
    FileChange(ForallFileChangeEventRequest),
    McpToolCall(ForallMcpToolCallEventRequest),
    DynamicToolCall(ForallDynamicToolCallEventRequest),
    CollabAgentToolCall(ForallCollabAgentToolCallEventRequest),
    WebSearch(ForallWebSearchEventRequest),
    ImageGeneration(ForallImageGenerationEventRequest),
    AcceptedLineFingerprints(Box<ForallAcceptedLineFingerprintsEventRequest>),
    #[allow(dead_code)]
    ReviewEvent(ForallReviewEventRequest),
    PluginUsed(ForallPluginUsedEventRequest),
    PluginInstalled(ForallPluginEventRequest),
    PluginUninstalled(ForallPluginEventRequest),
    PluginEnabled(ForallPluginEventRequest),
    PluginDisabled(ForallPluginEventRequest),
    PluginInstallFailed(ForallPluginInstallFailedEventRequest),
    ExternalAgentConfigImportCompleted(ForallOnboardingExternalAgentImportCompleteEventRequest),
    ExternalAgentConfigImportFailure(ForallOnboardingExternalAgentImportFailureEventRequest),
    ForallCheckCompleted(ForallCheckCompletedEventRequest),
    VerifiedGateBlocked(VerifiedGateBlockedEventRequest),
}

impl TrackEventRequest {
    pub(crate) fn should_send_in_isolated_request(&self) -> bool {
        matches!(self, Self::AcceptedLineFingerprints(_))
    }
}

#[derive(Serialize)]
pub(crate) struct ForallAcceptedLineFingerprintsEventParams {
    pub(crate) event_type: &'static str,
    pub(crate) turn_id: String,
    pub(crate) thread_id: String,
    pub(crate) product_surface: Option<String>,
    pub(crate) model_slug: Option<String>,
    pub(crate) completed_at: u64,
    pub(crate) repo_hash: Option<String>,
    pub(crate) accepted_added_lines: u64,
    pub(crate) accepted_deleted_lines: u64,
    pub(crate) line_fingerprints: Vec<AcceptedLineFingerprint>,
}

#[derive(Serialize)]
pub(crate) struct ForallAcceptedLineFingerprintsEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallAcceptedLineFingerprintsEventParams,
}

#[derive(Serialize)]
pub(crate) struct SkillInvocationEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) skill_id: String,
    pub(crate) skill_name: String,
    pub(crate) event_params: SkillInvocationEventParams,
}

#[derive(Serialize)]
pub(crate) struct SkillInvocationEventParams {
    pub(crate) product_client_id: Option<String>,
    pub(crate) skill_scope: Option<String>,
    pub(crate) plugin_id: Option<String>,
    pub(crate) repo_url: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) invoke_type: Option<InvocationType>,
    pub(crate) model_slug: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ForallAppServerClientMetadata {
    pub(crate) product_client_id: String,
    pub(crate) client_name: Option<String>,
    pub(crate) client_version: Option<String>,
    pub(crate) rpc_transport: AppServerRpcTransport,
    pub(crate) experimental_api_enabled: Option<bool>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ForallRuntimeMetadata {
    pub(crate) forall_rs_version: String,
    pub(crate) runtime_os: String,
    pub(crate) runtime_os_version: String,
    pub(crate) runtime_arch: String,
}

#[derive(Serialize)]
pub(crate) struct ThreadInitializedEventParams {
    pub(crate) thread_id: String,
    pub(crate) session_id: String,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) model: String,
    pub(crate) ephemeral: bool,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) initialization_mode: ThreadInitializationMode,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) forked_from_thread_id: Option<String>,
    pub(crate) created_at: u64,
}

#[derive(Serialize)]
pub(crate) struct ThreadInitializedEvent {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ThreadInitializedEventParams,
}

#[derive(Serialize)]
pub(crate) struct GuardianReviewEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: GuardianReviewEventPayload,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianReviewDecision {
    Approved,
    Denied,
    Aborted,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianReviewTerminalStatus {
    Approved,
    Denied,
    Aborted,
    TimedOut,
    FailedClosed,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianReviewFailureReason {
    Timeout,
    Cancelled,
    PromptBuildError,
    SessionError,
    ParseError,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianReviewSessionKind {
    TrunkNew,
    TrunkReused,
    EphemeralForked,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardianApprovalRequestSource {
    /// Approval requested directly by the main Forall turn.
    MainTurn,
    /// Approval requested by a delegated subagent and routed through the parent
    /// session for guardian review.
    DelegatedSubagent,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuardianReviewedAction {
    Shell {
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
    },
    UnifiedExec {
        sandbox_permissions: SandboxPermissions,
        additional_permissions: Option<AdditionalPermissionProfile>,
        tty: bool,
    },
    Execve {
        source: GuardianCommandSource,
        program: String,
        additional_permissions: Option<AdditionalPermissionProfile>,
    },
    ApplyPatch {},
    NetworkAccess {
        protocol: NetworkApprovalProtocol,
        port: u16,
    },
    McpToolCall {
        server: String,
        tool_name: String,
        connector_id: Option<String>,
        connector_name: Option<String>,
        tool_title: Option<String>,
    },
    RequestPermissions {},
}

#[derive(Clone, Serialize)]
pub struct GuardianReviewEventParams {
    pub thread_id: String,
    pub turn_id: String,
    pub review_id: String,
    pub target_item_id: Option<String>,
    pub approval_request_source: GuardianApprovalRequestSource,
    pub reviewed_action: GuardianReviewedAction,
    pub reviewed_action_truncated: bool,
    pub decision: GuardianReviewDecision,
    pub terminal_status: GuardianReviewTerminalStatus,
    pub failure_reason: Option<GuardianReviewFailureReason>,
    pub attempt_count: i64,
    pub risk_level: Option<GuardianRiskLevel>,
    pub user_authorization: Option<GuardianUserAuthorization>,
    pub outcome: Option<GuardianAssessmentOutcome>,
    pub guardian_thread_id: Option<String>,
    pub guardian_session_kind: Option<GuardianReviewSessionKind>,
    pub guardian_model: Option<String>,
    pub guardian_reasoning_effort: Option<String>,
    pub guardian_default_review_model_id: Option<String>,
    pub guardian_catalog_contains_auto_review: Option<bool>,
    pub guardian_review_model_overridden: Option<bool>,
    pub guardian_review_model_override: Option<String>,
    pub guardian_model_provider_id: Option<String>,
    pub had_prior_review_context: Option<bool>,
    pub review_timeout_ms: u64,
    pub tool_call_count: Option<u64>,
    pub time_to_first_token_ms: Option<u64>,
    pub completion_latency_ms: Option<u64>,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_output_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
}

pub struct GuardianReviewTrackContext {
    thread_id: String,
    turn_id: String,
    review_id: String,
    target_item_id: Option<String>,
    approval_request_source: GuardianApprovalRequestSource,
    reviewed_action: GuardianReviewedAction,
    review_timeout_ms: u64,
    pub started_at_ms: u64,
    started_instant: Instant,
}

impl GuardianReviewTrackContext {
    pub fn new(
        thread_id: String,
        turn_id: String,
        review_id: String,
        target_item_id: Option<String>,
        approval_request_source: GuardianApprovalRequestSource,
        reviewed_action: GuardianReviewedAction,
        review_timeout_ms: u64,
    ) -> Self {
        Self {
            thread_id,
            turn_id,
            review_id,
            target_item_id,
            approval_request_source,
            reviewed_action,
            review_timeout_ms,
            started_at_ms: now_unix_millis(),
            started_instant: Instant::now(),
        }
    }

    pub(crate) fn event_params(
        &self,
        result: GuardianReviewAnalyticsResult,
        completed_at_ms: u64,
    ) -> GuardianReviewEventParams {
        GuardianReviewEventParams {
            thread_id: self.thread_id.clone(),
            turn_id: self.turn_id.clone(),
            review_id: self.review_id.clone(),
            target_item_id: self.target_item_id.clone(),
            approval_request_source: self.approval_request_source,
            reviewed_action: self.reviewed_action.clone(),
            reviewed_action_truncated: result.reviewed_action_truncated,
            decision: result.decision,
            terminal_status: result.terminal_status,
            failure_reason: result.failure_reason,
            attempt_count: result.attempt_count,
            risk_level: result.risk_level,
            user_authorization: result.user_authorization,
            outcome: result.outcome,
            guardian_thread_id: result.guardian_thread_id,
            guardian_session_kind: result.guardian_session_kind,
            guardian_model: result.guardian_model,
            guardian_reasoning_effort: result.guardian_reasoning_effort,
            guardian_default_review_model_id: result.guardian_default_review_model_id,
            guardian_catalog_contains_auto_review: result.guardian_catalog_contains_auto_review,
            guardian_review_model_overridden: result.guardian_review_model_overridden,
            guardian_review_model_override: result.guardian_review_model_override,
            guardian_model_provider_id: result.guardian_model_provider_id,
            had_prior_review_context: result.had_prior_review_context,
            review_timeout_ms: self.review_timeout_ms,
            // TODO(rhan-oai): plumb nested Guardian review session tool-call counts.
            tool_call_count: None,
            time_to_first_token_ms: result.time_to_first_token_ms,
            completion_latency_ms: Some(self.started_instant.elapsed().as_millis() as u64),
            started_at: self.started_at_ms / 1_000,
            completed_at: Some(completed_at_ms / 1_000),
            input_tokens: result.token_usage.as_ref().map(|usage| usage.input_tokens),
            cached_input_tokens: result
                .token_usage
                .as_ref()
                .map(|usage| usage.cached_input_tokens),
            output_tokens: result.token_usage.as_ref().map(|usage| usage.output_tokens),
            reasoning_output_tokens: result
                .token_usage
                .as_ref()
                .map(|usage| usage.reasoning_output_tokens),
            total_tokens: result.token_usage.as_ref().map(|usage| usage.total_tokens),
        }
    }
}

#[derive(Debug)]
pub struct GuardianReviewAnalyticsResult {
    pub decision: GuardianReviewDecision,
    pub terminal_status: GuardianReviewTerminalStatus,
    pub failure_reason: Option<GuardianReviewFailureReason>,
    pub attempt_count: i64,
    pub risk_level: Option<GuardianRiskLevel>,
    pub user_authorization: Option<GuardianUserAuthorization>,
    pub outcome: Option<GuardianAssessmentOutcome>,
    pub guardian_thread_id: Option<String>,
    pub guardian_session_kind: Option<GuardianReviewSessionKind>,
    pub guardian_model: Option<String>,
    pub guardian_reasoning_effort: Option<String>,
    pub guardian_default_review_model_id: Option<String>,
    pub guardian_catalog_contains_auto_review: Option<bool>,
    pub guardian_review_model_overridden: Option<bool>,
    pub guardian_review_model_override: Option<String>,
    pub guardian_model_provider_id: Option<String>,
    pub had_prior_review_context: Option<bool>,
    pub reviewed_action_truncated: bool,
    pub token_usage: Option<TokenUsage>,
    pub time_to_first_token_ms: Option<u64>,
}

impl GuardianReviewAnalyticsResult {
    pub fn without_session() -> Self {
        Self {
            decision: GuardianReviewDecision::Denied,
            terminal_status: GuardianReviewTerminalStatus::FailedClosed,
            failure_reason: None,
            attempt_count: 1,
            risk_level: None,
            user_authorization: None,
            outcome: None,
            guardian_thread_id: None,
            guardian_session_kind: None,
            guardian_model: None,
            guardian_reasoning_effort: None,
            guardian_default_review_model_id: None,
            guardian_catalog_contains_auto_review: None,
            guardian_review_model_overridden: None,
            guardian_review_model_override: None,
            guardian_model_provider_id: None,
            had_prior_review_context: None,
            reviewed_action_truncated: false,
            token_usage: None,
            time_to_first_token_ms: None,
        }
    }

    pub fn from_session(params: GuardianReviewSessionAnalyticsParams) -> Self {
        Self {
            guardian_thread_id: Some(params.guardian_thread_id),
            guardian_session_kind: Some(params.guardian_session_kind),
            guardian_model: Some(params.guardian_model),
            guardian_reasoning_effort: params.guardian_reasoning_effort,
            guardian_default_review_model_id: Some(params.guardian_default_review_model_id),
            guardian_catalog_contains_auto_review: Some(
                params.guardian_catalog_contains_auto_review,
            ),
            guardian_review_model_overridden: Some(params.guardian_review_model_overridden),
            guardian_review_model_override: params.guardian_review_model_override,
            guardian_model_provider_id: Some(params.guardian_model_provider_id),
            had_prior_review_context: Some(params.had_prior_review_context),
            ..Self::without_session()
        }
    }
}

pub struct GuardianReviewSessionAnalyticsParams {
    pub guardian_thread_id: String,
    pub guardian_session_kind: GuardianReviewSessionKind,
    pub guardian_model: String,
    pub guardian_reasoning_effort: Option<String>,
    pub guardian_default_review_model_id: String,
    pub guardian_catalog_contains_auto_review: bool,
    pub guardian_review_model_overridden: bool,
    pub guardian_review_model_override: Option<String>,
    pub guardian_model_provider_id: String,
    pub had_prior_review_context: bool,
}

#[derive(Serialize)]
pub(crate) struct GuardianReviewEventPayload {
    pub(crate) session_id: String,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    #[serde(flatten)]
    pub(crate) guardian_review: GuardianReviewEventParams,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FinalApprovalOutcome {
    Unknown,
    NotNeeded,
    ConfigAllowed,
    PolicyForbidden,
    GuardianApproved,
    GuardianDenied,
    GuardianAborted,
    UserApproved,
    UserApprovedForSession,
    UserDenied,
    UserAborted,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolItemTerminalStatus {
    Completed,
    Failed,
    Rejected,
    Interrupted,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolItemFailureKind {
    ToolError,
    ApprovalDenied,
    ApprovalAborted,
    SandboxDenied,
    PolicyForbidden,
}

#[derive(Serialize)]
pub(crate) struct ForallToolItemEventBase {
    pub(crate) thread_id: String,
    pub(crate) turn_id: String,
    /// App-server ThreadItem.id. For tool-originated items this generally
    /// corresponds to the originating core call_id.
    pub(crate) item_id: String,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) tool_name: String,
    pub(crate) started_at_ms: u64,
    pub(crate) completed_at_ms: u64,
    // Observed item lifecycle duration. This may undercount end-to-end execution
    // for tools where app-server only sees part of the upstream flow.
    pub(crate) duration_ms: Option<u64>,
    pub(crate) execution_duration_ms: Option<u64>,
    pub(crate) review_count: u64,
    pub(crate) guardian_review_count: u64,
    pub(crate) user_review_count: u64,
    pub(crate) final_approval_outcome: FinalApprovalOutcome,
    pub(crate) terminal_status: ToolItemTerminalStatus,
    pub(crate) failure_kind: Option<ToolItemFailureKind>,
    pub(crate) requested_additional_permissions: bool,
    pub(crate) requested_network_access: bool,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewSubjectKind {
    CommandExecution,
    FileChange,
    McpToolCall,
    Permissions,
    NetworkAccess,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reviewer {
    Guardian,
    User,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewTrigger {
    Initial,
    SandboxDenial,
    NetworkPolicyDenial,
    ExecveIntercept,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewStatus {
    Approved,
    Denied,
    Aborted,
    TimedOut,
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewResolution {
    None,
    SessionApproval,
    ExecPolicyAmendment,
    NetworkPolicyAmendment,
}

#[derive(Serialize)]
pub(crate) struct ForallReviewEventParams {
    pub(crate) thread_id: String,
    pub(crate) turn_id: String,
    pub(crate) item_id: Option<String>,
    pub(crate) review_id: String,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) subject_kind: ReviewSubjectKind,
    pub(crate) subject_name: String,
    pub(crate) reviewer: Reviewer,
    pub(crate) trigger: ReviewTrigger,
    pub(crate) status: ReviewStatus,
    pub(crate) resolution: ReviewResolution,
    pub(crate) started_at_ms: u64,
    pub(crate) completed_at_ms: u64,
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallReviewEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallReviewEventParams,
}
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WebSearchActionKind {
    Search,
    OpenPage,
    FindInPage,
    Other,
}

#[derive(Serialize)]
pub(crate) struct ForallCommandExecutionEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) command_execution_source: CommandExecutionSource,
    pub(crate) exit_code: Option<i32>,
    pub(crate) command_total_action_count: u64,
    pub(crate) command_read_action_count: u64,
    pub(crate) command_list_files_action_count: u64,
    pub(crate) command_search_action_count: u64,
    pub(crate) command_unknown_action_count: u64,
}

#[derive(Serialize)]
pub(crate) struct ForallCommandExecutionEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallCommandExecutionEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallFileChangeEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) file_change_count: u64,
    pub(crate) file_add_count: u64,
    pub(crate) file_update_count: u64,
    pub(crate) file_delete_count: u64,
    pub(crate) file_move_count: u64,
}

#[derive(Serialize)]
pub(crate) struct ForallFileChangeEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallFileChangeEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallMcpToolCallEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) mcp_server_name: String,
    pub(crate) mcp_tool_name: String,
    pub(crate) mcp_error_present: bool,
    pub(crate) plugin_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallMcpToolCallEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallMcpToolCallEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallDynamicToolCallEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) dynamic_tool_name: String,
    pub(crate) success: Option<bool>,
    pub(crate) output_content_item_count: Option<u64>,
    pub(crate) output_text_item_count: Option<u64>,
    pub(crate) output_image_item_count: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallDynamicToolCallEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallDynamicToolCallEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallCollabAgentToolCallEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) sender_thread_id: String,
    pub(crate) receiver_thread_count: u64,
    pub(crate) receiver_thread_ids: Option<Vec<String>>,
    pub(crate) requested_model: Option<String>,
    pub(crate) requested_reasoning_effort: Option<String>,
    pub(crate) agent_state_count: Option<u64>,
    pub(crate) completed_agent_count: Option<u64>,
    pub(crate) failed_agent_count: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallCollabAgentToolCallEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallCollabAgentToolCallEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallWebSearchEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) web_search_action: Option<WebSearchActionKind>,
    pub(crate) query_present: bool,
    pub(crate) query_count: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallWebSearchEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallWebSearchEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallImageGenerationEventParams {
    #[serde(flatten)]
    pub(crate) base: ForallToolItemEventBase,
    pub(crate) revised_prompt_present: bool,
    pub(crate) saved_path_present: bool,
}

#[derive(Serialize)]
pub(crate) struct ForallImageGenerationEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallImageGenerationEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallAppMetadata {
    pub(crate) connector_id: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) app_name: Option<String>,
    pub(crate) product_client_id: Option<String>,
    pub(crate) invoke_type: Option<InvocationType>,
    pub(crate) model_slug: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallAppMentionedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallAppMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallAppUsedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallAppMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallHookRunMetadata {
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) model_slug: Option<String>,
    pub(crate) hook_name: Option<String>,
    pub(crate) hook_source: Option<&'static str>,
    pub(crate) status: Option<HookRunStatus>,
}

#[derive(Serialize)]
pub(crate) struct ForallHookRunEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallHookRunMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallCheckCompletedMetadata {
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) model_slug: Option<String>,
    pub(crate) change: Option<String>,
    pub(crate) ok: bool,
    pub(crate) strict: bool,
    pub(crate) duration_ms: u64,
    pub(crate) phases_pass: u32,
    pub(crate) phases_fail: u32,
    pub(crate) phases_skip: u32,
    pub(crate) critical_issues: u32,
    pub(crate) warning_issues: u32,
    pub(crate) total_requirements: u32,
    pub(crate) source: String,
    pub(crate) completed_at: u64,
}

#[derive(Serialize)]
pub(crate) struct ForallCheckCompletedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallCheckCompletedMetadata,
}

#[derive(Serialize)]
pub(crate) struct VerifiedGateBlockedMetadata {
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) model_slug: Option<String>,
    pub(crate) tool: String,
    pub(crate) code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) active_change: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) blocked_by: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_artifact: Option<String>,
    pub(crate) blocked_at: u64,
}

#[derive(Serialize)]
pub(crate) struct VerifiedGateBlockedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: VerifiedGateBlockedMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallCompactionEventParams {
    pub(crate) thread_id: String,
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) trigger: CompactionTrigger,
    pub(crate) reason: CompactionReason,
    pub(crate) implementation: CompactionImplementation,
    pub(crate) phase: CompactionPhase,
    pub(crate) strategy: CompactionStrategy,
    pub(crate) status: CompactionStatus,
    pub(crate) forall_error_kind: Option<ForallErrKind>,
    pub(crate) forall_error_http_status_code: Option<u16>,
    pub(crate) active_context_tokens_before: i64,
    pub(crate) active_context_tokens_after: i64,
    pub(crate) retained_image_count: Option<usize>,
    pub(crate) compaction_summary_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
    pub(crate) started_at: u64,
    pub(crate) completed_at: u64,
    pub(crate) duration_ms: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallCompactionEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallCompactionEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallGoalEventParams {
    pub(crate) thread_id: String,
    pub(crate) session_id: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) goal_id: String,
    pub(crate) event_kind: GoalEventKind,
    pub(crate) goal_status: forall_state::ThreadGoalStatus,
    pub(crate) has_token_budget: bool,
    pub(crate) cumulative_tokens_accounted: Option<i64>,
    pub(crate) cumulative_time_accounted_seconds: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct ForallGoalEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallGoalEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallTurnEventParams {
    pub(crate) thread_id: String,
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    // TODO(rhan-oai): Populate once queued/default submission type is plumbed from
    // the turn/start callsites instead of always being reported as None.
    pub(crate) submission_type: Option<TurnSubmissionType>,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) ephemeral: bool,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) initialization_mode: ThreadInitializationMode,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) model_provider: String,
    pub(crate) sandbox_policy: Option<&'static str>,
    pub(crate) reasoning_effort: Option<String>,
    pub(crate) reasoning_summary: Option<String>,
    pub(crate) service_tier: String,
    pub(crate) approval_policy: String,
    pub(crate) approvals_reviewer: String,
    pub(crate) sandbox_network_access: bool,
    pub(crate) collaboration_mode: Option<&'static str>,
    pub(crate) personality: Option<String>,
    pub(crate) workspace_kind: Option<String>,
    pub(crate) num_input_images: usize,
    pub(crate) is_first_turn: bool,
    pub(crate) status: Option<TurnStatus>,
    pub(crate) turn_error: Option<ForallErrorInfo>,
    pub(crate) forall_error_kind: Option<ForallErrKind>,
    pub(crate) forall_error_http_status_code: Option<u16>,
    pub(crate) steer_count: Option<usize>,
    pub(crate) total_tool_call_count: Option<usize>,
    pub(crate) shell_command_count: Option<usize>,
    pub(crate) file_change_count: Option<usize>,
    pub(crate) mcp_tool_call_count: Option<usize>,
    pub(crate) dynamic_tool_call_count: Option<usize>,
    pub(crate) specs_tool_call_count: Option<usize>,
    pub(crate) subagent_tool_call_count: Option<usize>,
    pub(crate) web_search_count: Option<usize>,
    pub(crate) image_generation_count: Option<usize>,
    pub(crate) input_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) reasoning_output_tokens: Option<i64>,
    pub(crate) total_tokens: Option<i64>,
    pub(crate) before_first_sampling_ms: u64,
    pub(crate) sampling_ms: u64,
    pub(crate) between_sampling_overhead_ms: u64,
    pub(crate) tool_blocking_ms: u64,
    pub(crate) after_last_sampling_ms: u64,
    pub(crate) sampling_request_count: u32,
    pub(crate) sampling_retry_count: u32,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) started_at: Option<u64>,
    pub(crate) completed_at: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct ForallTurnEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallTurnEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallTurnSteerEventParams {
    pub(crate) thread_id: String,
    pub(crate) session_id: String,
    pub(crate) expected_turn_id: Option<String>,
    pub(crate) accepted_turn_id: Option<String>,
    pub(crate) app_server_client: ForallAppServerClientMetadata,
    pub(crate) runtime: ForallRuntimeMetadata,
    pub(crate) thread_source: Option<ThreadSource>,
    pub(crate) subagent_source: Option<String>,
    pub(crate) parent_thread_id: Option<String>,
    pub(crate) num_input_images: usize,
    pub(crate) result: TurnSteerResult,
    pub(crate) rejection_reason: Option<TurnSteerRejectionReason>,
    pub(crate) created_at: u64,
}

#[derive(Serialize)]
pub(crate) struct ForallTurnSteerEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallTurnSteerEventParams,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginMetadata {
    pub(crate) plugin_id: Option<String>,
    pub(crate) plugin_name: Option<String>,
    pub(crate) marketplace_name: Option<String>,
    pub(crate) has_skills: Option<bool>,
    pub(crate) mcp_server_count: Option<usize>,
    pub(crate) product_client_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginUsedMetadata {
    #[serde(flatten)]
    pub(crate) plugin: ForallPluginMetadata,
    pub(crate) mcp_server_names: Option<Vec<String>>,
    pub(crate) thread_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) model_slug: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallPluginMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginInstallFailedMetadata {
    #[serde(flatten)]
    pub(crate) plugin: ForallPluginMetadata,
    pub(crate) error_type: String,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginInstallFailedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallPluginInstallFailedMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallOnboardingExternalAgentImportCompleteMetadata {
    pub(crate) import_id: String,
    pub(crate) source: String,
    #[serde(rename = "type")]
    pub(crate) item_type: String,
    pub(crate) success_count: usize,
    pub(crate) failed_count: usize,
    pub(crate) product_client_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallOnboardingExternalAgentImportCompleteEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallOnboardingExternalAgentImportCompleteMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallOnboardingExternalAgentImportFailureMetadata {
    pub(crate) import_id: String,
    pub(crate) source: String,
    #[serde(rename = "type")]
    pub(crate) item_type: String,
    pub(crate) failure_stage: String,
    pub(crate) error_type: String,
    pub(crate) product_client_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct ForallOnboardingExternalAgentImportFailureEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallOnboardingExternalAgentImportFailureMetadata,
}

#[derive(Serialize)]
pub(crate) struct ForallPluginUsedEventRequest {
    pub(crate) event_type: &'static str,
    pub(crate) event_params: ForallPluginUsedMetadata,
}

pub(crate) fn plugin_state_event_type(state: PluginState) -> &'static str {
    match state {
        PluginState::Installed => "forall_plugin_installed",
        PluginState::Uninstalled => "forall_plugin_uninstalled",
        PluginState::Enabled => "forall_plugin_enabled",
        PluginState::Disabled => "forall_plugin_disabled",
    }
}

pub(crate) fn forall_app_metadata(
    tracking: &TrackEventsContext,
    app: AppInvocation,
) -> ForallAppMetadata {
    ForallAppMetadata {
        connector_id: app.connector_id,
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        app_name: app.app_name,
        product_client_id: Some(originator().value),
        invoke_type: app.invocation_type,
        model_slug: Some(tracking.model_slug.clone()),
    }
}

pub(crate) fn forall_plugin_metadata(plugin: PluginTelemetryMetadata) -> ForallPluginMetadata {
    let PluginTelemetryMetadata {
        plugin_id,
        remote_plugin_id,
        capability_summary,
    } = plugin;
    let event_plugin_id = remote_plugin_id.unwrap_or_else(|| plugin_id.as_key());
    ForallPluginMetadata {
        plugin_id: Some(event_plugin_id),
        plugin_name: Some(plugin_id.plugin_name),
        marketplace_name: Some(plugin_id.marketplace_name),
        has_skills: capability_summary
            .as_ref()
            .map(|summary| summary.has_skills),
        mcp_server_count: capability_summary
            .as_ref()
            .map(|summary| summary.mcp_server_names.len()),
        product_client_id: Some(originator().value),
    }
}

pub(crate) fn forall_compaction_event_params(
    input: ForallCompactionEvent,
    session_id: String,
    app_server_client: ForallAppServerClientMetadata,
    runtime: ForallRuntimeMetadata,
    thread_source: Option<ThreadSource>,
    subagent_source: Option<String>,
    parent_thread_id: Option<String>,
) -> ForallCompactionEventParams {
    ForallCompactionEventParams {
        thread_id: input.thread_id,
        session_id,
        turn_id: input.turn_id,
        app_server_client,
        runtime,
        thread_source,
        subagent_source,
        parent_thread_id,
        trigger: input.trigger,
        reason: input.reason,
        implementation: input.implementation,
        phase: input.phase,
        strategy: input.strategy,
        status: input.status,
        forall_error_kind: input.forall_error_kind,
        forall_error_http_status_code: input.forall_error_http_status_code,
        active_context_tokens_before: input.active_context_tokens_before,
        active_context_tokens_after: input.active_context_tokens_after,
        retained_image_count: input.retained_image_count,
        compaction_summary_tokens: input.compaction_summary_tokens,
        cached_input_tokens: input.cached_input_tokens,
        started_at: input.started_at,
        completed_at: input.completed_at,
        duration_ms: input.duration_ms,
    }
}

pub(crate) fn forall_goal_event_params(
    input: ForallGoalEvent,
    session_id: String,
    app_server_client: ForallAppServerClientMetadata,
    runtime: ForallRuntimeMetadata,
    thread_source: Option<ThreadSource>,
    subagent_source: Option<String>,
    parent_thread_id: Option<String>,
) -> ForallGoalEventParams {
    ForallGoalEventParams {
        thread_id: input.thread_id,
        session_id,
        turn_id: input.turn_id,
        app_server_client,
        runtime,
        thread_source,
        subagent_source,
        parent_thread_id,
        goal_id: input.goal_id,
        event_kind: input.event_kind,
        goal_status: input.goal_status,
        has_token_budget: input.has_token_budget,
        cumulative_tokens_accounted: input.cumulative_tokens_accounted,
        cumulative_time_accounted_seconds: input.cumulative_time_accounted_seconds,
    }
}

pub(crate) fn forall_plugin_used_metadata(
    tracking: &TrackEventsContext,
    plugin: PluginTelemetryMetadata,
) -> ForallPluginUsedMetadata {
    let mcp_server_names = plugin
        .capability_summary
        .as_ref()
        .map(|summary| summary.mcp_server_names.clone());
    ForallPluginUsedMetadata {
        plugin: forall_plugin_metadata(plugin),
        mcp_server_names,
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        model_slug: Some(tracking.model_slug.clone()),
    }
}

pub(crate) fn forall_hook_run_metadata(
    tracking: &TrackEventsContext,
    hook: HookRunFact,
) -> ForallHookRunMetadata {
    ForallHookRunMetadata {
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        model_slug: Some(tracking.model_slug.clone()),
        hook_name: Some(analytics_hook_event_name(hook.event_name).to_owned()),
        hook_source: Some(analytics_hook_source(hook.hook_source)),
        status: Some(analytics_hook_status(hook.status)),
    }
}

pub(crate) fn forall_check_completed_metadata(
    tracking: &TrackEventsContext,
    check: ForallCheckCompletedFact,
) -> ForallCheckCompletedMetadata {
    ForallCheckCompletedMetadata {
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        model_slug: Some(tracking.model_slug.clone()),
        change: check.change,
        ok: check.ok,
        strict: check.strict,
        duration_ms: check.duration_ms,
        phases_pass: check.phases_pass,
        phases_fail: check.phases_fail,
        phases_skip: check.phases_skip,
        critical_issues: check.critical_issues,
        warning_issues: check.warning_issues,
        total_requirements: check.total_requirements,
        source: check.source,
        completed_at: now_unix_millis(),
    }
}

pub(crate) fn verified_gate_blocked_metadata(
    tracking: &TrackEventsContext,
    gate: VerifiedGateBlockedFact,
) -> VerifiedGateBlockedMetadata {
    VerifiedGateBlockedMetadata {
        thread_id: Some(tracking.thread_id.clone()),
        turn_id: Some(tracking.turn_id.clone()),
        model_slug: Some(tracking.model_slug.clone()),
        tool: gate.tool,
        code: gate.code,
        paths: gate.paths,
        active_change: gate.active_change,
        blocked_by: gate.blocked_by,
        next_artifact: gate.next_artifact,
        blocked_at: now_unix_millis(),
    }
}

fn analytics_hook_event_name(event_name: HookEventName) -> &'static str {
    match event_name {
        HookEventName::PreToolUse => "PreToolUse",
        HookEventName::PermissionRequest => "PermissionRequest",
        HookEventName::PostToolUse => "PostToolUse",
        HookEventName::PreCompact => "PreCompact",
        HookEventName::PostCompact => "PostCompact",
        HookEventName::SessionStart => "SessionStart",
        HookEventName::UserPromptSubmit => "UserPromptSubmit",
        HookEventName::SubagentStart => "SubagentStart",
        HookEventName::SubagentStop => "SubagentStop",
        HookEventName::Stop => "Stop",
    }
}

fn analytics_hook_source(source: HookSource) -> &'static str {
    match source {
        HookSource::System => "system",
        HookSource::User => "user",
        HookSource::Project => "project",
        HookSource::Mdm => "mdm",
        HookSource::SessionFlags => "session_flags",
        HookSource::Plugin => "plugin",
        HookSource::CloudRequirements => "cloud_requirements",
        HookSource::CloudManagedConfig => "cloud_managed_config",
        HookSource::LegacyManagedConfigFile => "legacy_managed_config_file",
        HookSource::LegacyManagedConfigMdm => "legacy_managed_config_mdm",
        HookSource::Unknown => "unknown",
    }
}

pub(crate) fn current_runtime_metadata() -> ForallRuntimeMetadata {
    let os_info = os_info::get();
    ForallRuntimeMetadata {
        forall_rs_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_os: std::env::consts::OS.to_string(),
        runtime_os_version: os_info.version().to_string(),
        runtime_arch: std::env::consts::ARCH.to_string(),
    }
}

pub(crate) fn subagent_thread_started_event_request(
    input: SubAgentThreadStartedInput,
) -> ThreadInitializedEvent {
    let event_params = ThreadInitializedEventParams {
        thread_id: input.thread_id,
        session_id: input.session_id,
        app_server_client: ForallAppServerClientMetadata {
            product_client_id: input.product_client_id,
            client_name: Some(input.client_name),
            client_version: Some(input.client_version),
            rpc_transport: AppServerRpcTransport::InProcess,
            experimental_api_enabled: None,
        },
        runtime: current_runtime_metadata(),
        model: input.model,
        ephemeral: input.ephemeral,
        thread_source: Some(ThreadSource::Subagent),
        initialization_mode: ThreadInitializationMode::New,
        subagent_source: Some(subagent_source_name(&input.subagent_source)),
        parent_thread_id: input.parent_thread_id,
        forked_from_thread_id: input.forked_from_thread_id,
        created_at: input.created_at,
    };
    ThreadInitializedEvent {
        event_type: "forall_thread_initialized",
        event_params,
    }
}

pub(crate) fn subagent_source_name(subagent_source: &SubAgentSource) -> String {
    subagent_source.kind().to_string()
}

fn analytics_hook_status(status: HookRunStatus) -> HookRunStatus {
    match status {
        // Running is unexpected here and normalized defensively.
        HookRunStatus::Running => HookRunStatus::Failed,
        other => other,
    }
}
