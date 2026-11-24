use forall_protocol::config_types::ApprovalsReviewer;
use forall_protocol::config_types::ModeKind;
use forall_protocol::config_types::Personality;
use forall_protocol::config_types::ReasoningSummary;
use forall_protocol::config_types::ServiceTier;
use forall_protocol::error::ForallErr;
use forall_protocol::models::PermissionProfile;
use forall_protocol::openai_models::ReasoningEffort;
use forall_protocol::protocol::AskForApproval;
use forall_protocol::protocol::HookEventName;
use forall_protocol::protocol::HookRunStatus;
use forall_protocol::protocol::HookSource;
use forall_protocol::protocol::SessionSource;
use forall_protocol::protocol::SkillScope;
use forall_protocol::protocol::SubAgentSource;
use forall_protocol::protocol::TokenUsage;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AcceptedLineFingerprint {
    pub path_hash: String,
    pub line_hash: String,
}

#[derive(Clone)]
pub struct TrackEventsContext {
    pub model_slug: String,
    pub thread_id: String,
    pub turn_id: String,
}

pub fn build_track_events_context(
    model_slug: String,
    thread_id: String,
    turn_id: String,
) -> TrackEventsContext {
    TrackEventsContext {
        model_slug,
        thread_id,
        turn_id,
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSubmissionType {
    Default,
    Queued,
}

#[derive(Clone)]
pub struct TurnResolvedConfigFact {
    pub turn_id: String,
    pub thread_id: String,
    pub num_input_images: usize,
    pub submission_type: Option<TurnSubmissionType>,
    pub ephemeral: bool,
    pub session_source: SessionSource,
    pub model: String,
    pub model_provider: String,
    pub permission_profile: PermissionProfile,
    pub permission_profile_cwd: PathBuf,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub service_tier: Option<ServiceTier>,
    pub approval_policy: AskForApproval,
    pub approvals_reviewer: ApprovalsReviewer,
    pub sandbox_network_access: bool,
    pub collaboration_mode: ModeKind,
    pub personality: Option<Personality>,
    pub workspace_kind: Option<String>,
    pub is_first_turn: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadInitializationMode {
    New,
    Forked,
    Resumed,
}

#[derive(Clone)]
pub struct TurnTokenUsageFact {
    pub turn_id: String,
    pub thread_id: String,
    pub token_usage: TokenUsage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TurnProfile {
    pub before_first_sampling_ms: u64,
    pub sampling_ms: u64,
    pub between_sampling_overhead_ms: u64,
    pub tool_blocking_ms: u64,
    pub after_last_sampling_ms: u64,
    pub sampling_request_count: u32,
    pub sampling_retry_count: u32,
}

#[derive(Clone)]
pub struct TurnProfileFact {
    pub turn_id: String,
    pub profile: TurnProfile,
}

#[derive(Clone)]
pub struct TurnForallErrorFact {
    pub(crate) turn_id: String,
    pub(crate) thread_id: String,
    pub(crate) error: TurnForallError,
}

impl TurnForallErrorFact {
    pub fn from_forall_err(thread_id: String, turn_id: String, error: &ForallErr) -> Self {
        Self {
            turn_id,
            thread_id,
            error: TurnForallError::from_forall_err(error),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForallErrKind {
    TurnAborted,
    Stream,
    ContextWindowExceeded,
    ThreadNotFound,
    AgentLimitReached,
    SessionConfiguredNotFirstEvent,
    Timeout,
    RequestTimeout,
    Spawn,
    Interrupted,
    UnexpectedStatus,
    InvalidRequest,
    InvalidImageRequest,
    UsageLimitReached,
    ServerOverloaded,
    CyberPolicy,
    ResponseStreamFailed,
    ConnectionFailed,
    QuotaExceeded,
    UsageNotIncluded,
    InternalServerError,
    RetryLimit,
    InternalAgentDied,
    Sandbox,
    LandlockSandboxExecutableNotProvided,
    UnsupportedOperation,
    RefreshTokenFailed,
    Fatal,
    Io,
    Json,
    #[cfg(target_os = "linux")]
    LandlockRuleset,
    #[cfg(target_os = "linux")]
    LandlockPathFd,
    TokioJoin,
    EnvVar,
}

#[derive(Clone)]
pub(crate) struct TurnForallError {
    pub(crate) kind: ForallErrKind,
    pub(crate) http_status_code: Option<u16>,
}

impl TurnForallError {
    fn from_forall_err(error: &ForallErr) -> Self {
        Self {
            kind: error.into(),
            http_status_code: error.http_status_code_value(),
        }
    }
}

impl From<&ForallErr> for ForallErrKind {
    fn from(error: &ForallErr) -> Self {
        match error {
            ForallErr::TurnAborted => ForallErrKind::TurnAborted,
            ForallErr::Stream(..) => ForallErrKind::Stream,
            ForallErr::ContextWindowExceeded => ForallErrKind::ContextWindowExceeded,
            ForallErr::ThreadNotFound(_) => ForallErrKind::ThreadNotFound,
            ForallErr::AgentLimitReached { .. } => ForallErrKind::AgentLimitReached,
            ForallErr::SessionConfiguredNotFirstEvent => {
                ForallErrKind::SessionConfiguredNotFirstEvent
            }
            ForallErr::Timeout => ForallErrKind::Timeout,
            ForallErr::RequestTimeout => ForallErrKind::RequestTimeout,
            ForallErr::Spawn => ForallErrKind::Spawn,
            ForallErr::Interrupted => ForallErrKind::Interrupted,
            ForallErr::UnexpectedStatus(_) => ForallErrKind::UnexpectedStatus,
            ForallErr::InvalidRequest(_) => ForallErrKind::InvalidRequest,
            ForallErr::InvalidImageRequest() => ForallErrKind::InvalidImageRequest,
            ForallErr::UsageLimitReached(_) => ForallErrKind::UsageLimitReached,
            ForallErr::ServerOverloaded => ForallErrKind::ServerOverloaded,
            ForallErr::CyberPolicy { .. } => ForallErrKind::CyberPolicy,
            ForallErr::ResponseStreamFailed(_) => ForallErrKind::ResponseStreamFailed,
            ForallErr::ConnectionFailed(_) => ForallErrKind::ConnectionFailed,
            ForallErr::QuotaExceeded => ForallErrKind::QuotaExceeded,
            ForallErr::UsageNotIncluded => ForallErrKind::UsageNotIncluded,
            ForallErr::InternalServerError => ForallErrKind::InternalServerError,
            ForallErr::RetryLimit(_) => ForallErrKind::RetryLimit,
            ForallErr::InternalAgentDied => ForallErrKind::InternalAgentDied,
            ForallErr::Sandbox(_) => ForallErrKind::Sandbox,
            ForallErr::LandlockSandboxExecutableNotProvided => {
                ForallErrKind::LandlockSandboxExecutableNotProvided
            }
            ForallErr::UnsupportedOperation(_) => ForallErrKind::UnsupportedOperation,
            ForallErr::RefreshTokenFailed(_) => ForallErrKind::RefreshTokenFailed,
            ForallErr::Fatal(_) => ForallErrKind::Fatal,
            ForallErr::Io(_) => ForallErrKind::Io,
            ForallErr::Json(_) => ForallErrKind::Json,
            #[cfg(target_os = "linux")]
            ForallErr::LandlockRuleset(_) => ForallErrKind::LandlockRuleset,
            #[cfg(target_os = "linux")]
            ForallErr::LandlockPathFd(_) => ForallErrKind::LandlockPathFd,
            ForallErr::TokioJoin(_) => ForallErrKind::TokioJoin,
            ForallErr::EnvVar(_) => ForallErrKind::EnvVar,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerResult {
    Accepted,
    Rejected,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnSteerRejectionReason {
    NoActiveTurn,
    ExpectedTurnMismatch,
    NonSteerableReview,
    NonSteerableCompact,
    EmptyInput,
    InputTooLarge,
}

#[derive(Clone)]
pub struct ForallTurnSteerEvent {
    pub expected_turn_id: Option<String>,
    pub accepted_turn_id: Option<String>,
    pub num_input_images: usize,
    pub result: TurnSteerResult,
    pub rejection_reason: Option<TurnSteerRejectionReason>,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum AnalyticsJsonRpcError {
    TurnSteer(TurnSteerRequestError),
    Input(InputError),
}

#[derive(Clone, Copy, Debug)]
pub enum TurnSteerRequestError {
    NoActiveTurn,
    ExpectedTurnMismatch,
    NonSteerableReview,
    NonSteerableCompact,
}

#[derive(Clone, Copy, Debug)]
pub enum InputError {
    Empty,
    TooLarge,
}

impl From<TurnSteerRequestError> for TurnSteerRejectionReason {
    fn from(error: TurnSteerRequestError) -> Self {
        match error {
            TurnSteerRequestError::NoActiveTurn => Self::NoActiveTurn,
            TurnSteerRequestError::ExpectedTurnMismatch => Self::ExpectedTurnMismatch,
            TurnSteerRequestError::NonSteerableReview => Self::NonSteerableReview,
            TurnSteerRequestError::NonSteerableCompact => Self::NonSteerableCompact,
        }
    }
}

impl From<InputError> for TurnSteerRejectionReason {
    fn from(error: InputError) -> Self {
        match error {
            InputError::Empty => Self::EmptyInput,
            InputError::TooLarge => Self::InputTooLarge,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub skill_scope: SkillScope,
    pub skill_path: PathBuf,
    pub plugin_id: Option<String>,
    pub invocation_type: InvocationType,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InvocationType {
    Explicit,
    Implicit,
}

pub struct AppInvocation {
    pub connector_id: Option<String>,
    pub app_name: Option<String>,
    pub invocation_type: Option<InvocationType>,
}

#[derive(Clone)]
pub struct SubAgentThreadStartedInput {
    pub session_id: String,
    pub thread_id: String,
    pub parent_thread_id: Option<String>,
    pub forked_from_thread_id: Option<String>,
    pub product_client_id: String,
    pub client_name: String,
    pub client_version: String,
    pub model: String,
    pub ephemeral: bool,
    pub subagent_source: SubAgentSource,
    pub created_at: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionReason {
    UserRequested,
    ContextLimit,
    ModelDownshift,
    CompHashChanged,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionImplementation {
    Responses,
    ResponsesCompactionV2,
    ResponsesCompact,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionPhase {
    StandaloneTurn,
    PreTurn,
    MidTurn,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStrategy {
    Memento,
    PrefixCompaction,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionStatus {
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone)]
pub struct ForallCompactionEvent {
    pub thread_id: String,
    pub turn_id: String,
    pub trigger: CompactionTrigger,
    pub reason: CompactionReason,
    pub implementation: CompactionImplementation,
    pub phase: CompactionPhase,
    pub strategy: CompactionStrategy,
    pub status: CompactionStatus,
    pub forall_error_kind: Option<ForallErrKind>,
    pub forall_error_http_status_code: Option<u16>,
    pub active_context_tokens_before: i64,
    pub active_context_tokens_after: i64,
    pub retained_image_count: Option<usize>,
    pub compaction_summary_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub started_at: u64,
    pub completed_at: u64,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEventKind {
    Created,
    UsageAccounted,
    StatusChanged,
    Cleared,
}

#[derive(Clone)]
pub struct ForallGoalEvent {
    pub thread_id: String,
    pub turn_id: Option<String>,
    pub goal_id: String,
    pub event_kind: GoalEventKind,
    pub goal_status: forall_state::ThreadGoalStatus,
    pub has_token_budget: bool,
    pub cumulative_tokens_accounted: Option<i64>,
    pub cumulative_time_accounted_seconds: Option<i64>,
}

pub struct HookRunFact {
    pub event_name: HookEventName,
    pub hook_source: HookSource,
    pub status: HookRunStatus,
}

pub struct ExternalAgentConfigImportCompletedInput {
    pub import_id: String,
    pub source: String,
    pub item_type: String,
    pub success_count: usize,
    pub failed_count: usize,
}

pub struct ExternalAgentConfigImportFailureInput {
    pub import_id: String,
    pub source: String,
    pub item_type: String,
    pub failure_stage: String,
    pub error_type: String,
}

/// Verified-engineering check completed (Phase 4 product analytics).
#[derive(Clone)]
pub struct ForallCheckCompletedInput {
    pub tracking: TrackEventsContext,
    pub check: ForallCheckCompletedFact,
}

#[derive(Clone)]
pub struct ForallCheckCompletedFact {
    pub change: Option<String>,
    pub ok: bool,
    pub strict: bool,
    pub duration_ms: u64,
    pub phases_pass: u32,
    pub phases_fail: u32,
    pub phases_skip: u32,
    pub critical_issues: u32,
    pub warning_issues: u32,
    pub total_requirements: u32,
    pub source: String,
}

/// VerifiedGate blocked a tool mutation (Phase 4 product analytics).
#[derive(Clone)]
pub struct VerifiedGateBlockedInput {
    pub tracking: TrackEventsContext,
    pub gate: VerifiedGateBlockedFact,
}

#[derive(Clone)]
pub struct VerifiedGateBlockedFact {
    pub tool: String,
    pub code: String,
    pub paths: Option<Vec<String>>,
    pub active_change: Option<String>,
    pub blocked_by: Option<Vec<String>>,
    pub next_artifact: Option<String>,
}
