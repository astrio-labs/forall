use crate::agent::AgentStatus;
use crate::config::ConstraintResult;
use crate::session::Forall;
use crate::session::SessionSettingsUpdate;
use crate::session::SteerInputError;
use forall_features::Feature;
use forall_otel::SessionTelemetry;
use forall_protocol::ThreadId;
use forall_protocol::config_types::ApprovalsReviewer;
use forall_protocol::config_types::CollaborationMode;
use forall_protocol::config_types::MultiAgentMode;
use forall_protocol::config_types::Personality;
use forall_protocol::config_types::ReasoningSummary;
use forall_protocol::config_types::WindowsSandboxLevel;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result as ForallResult;
use forall_protocol::mcp::CallToolResult;
use forall_protocol::models::ActivePermissionProfile;
use forall_protocol::models::ContentItem;
use forall_protocol::models::PermissionProfile;
use forall_protocol::models::ResponseItem;
use forall_protocol::openai_models::ReasoningEffort;
use forall_protocol::protocol::AdditionalContextEntry;
use forall_protocol::protocol::AskForApproval;
use forall_protocol::protocol::Event;
use forall_protocol::protocol::MultiAgentVersion;
use forall_protocol::protocol::Op;
use forall_protocol::protocol::RolloutItem;
use forall_protocol::protocol::SandboxPolicy;
use forall_protocol::protocol::SessionConfiguredEvent;
use forall_protocol::protocol::SessionSource;
use forall_protocol::protocol::Submission;
use forall_protocol::protocol::ThreadMemoryMode;
use forall_protocol::protocol::ThreadSource;
use forall_protocol::protocol::TokenUsageInfo;
use forall_protocol::protocol::TurnEnvironmentSelection;
use forall_protocol::protocol::TurnEnvironmentSelections;
use forall_protocol::protocol::W3cTraceContext;
use forall_protocol::user_input::UserInput;
use forall_thread_store::StoredThread;
use forall_thread_store::StoredThreadHistory;
use forall_thread_store::ThreadMetadataPatch;
use forall_thread_store::ThreadStoreError;
use forall_thread_store::ThreadStoreResult;
use forall_utils_absolute_path::AbsolutePathBuf;
use forall_utils_path_uri::LegacyAppPathString;
use forall_utils_path_uri::PathUri;
use rmcp::model::ReadResourceRequestParams;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::sync::watch;

use forall_rollout::state_db::StateDbHandle;

#[derive(Clone, Debug)]
pub struct ThreadConfigSnapshot {
    pub model: String,
    pub model_provider_id: String,
    pub service_tier: Option<String>,
    pub approval_policy: AskForApproval,
    pub approvals_reviewer: ApprovalsReviewer,
    pub permission_profile: PermissionProfile,
    pub active_permission_profile: Option<ActivePermissionProfile>,
    pub environments: TurnEnvironmentSelections,
    pub workspace_roots: Vec<AbsolutePathBuf>,
    pub profile_workspace_roots: Vec<AbsolutePathBuf>,
    pub ephemeral: bool,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub reasoning_summary: Option<ReasoningSummary>,
    pub personality: Option<Personality>,
    pub collaboration_mode: CollaborationMode,
    pub multi_agent_mode: MultiAgentMode,
    pub session_source: SessionSource,
    pub forked_from_thread_id: Option<ThreadId>,
    pub parent_thread_id: Option<ThreadId>,
    pub thread_source: Option<ThreadSource>,
}

/// Explains why `ForallThread::try_start_turn_if_idle` rejected an automatic
/// idle turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryStartTurnIfIdleRejectionReason {
    /// User/client-triggered mailbox work is already queued and must take
    /// priority over extension-initiated idle work.
    PendingTriggerTurn,
    /// The thread is in Plan mode, where automatic idle work must not start a
    /// new model turn.
    PlanMode,
    /// Another turn or task is active, or the idle reservation was lost before
    /// the automatic turn could start.
    Busy,
}

/// Rejection returned when an extension asks to start automatic idle work but
/// the thread is not eligible to run it.
#[derive(Debug)]
pub struct TryStartTurnIfIdleError {
    reason: TryStartTurnIfIdleRejectionReason,
    input: Vec<ResponseItem>,
}

impl TryStartTurnIfIdleError {
    pub(crate) fn new(reason: TryStartTurnIfIdleRejectionReason, input: Vec<ResponseItem>) -> Self {
        Self { reason, input }
    }

    /// Returns the stable reason the automatic idle turn was rejected.
    pub fn reason(&self) -> TryStartTurnIfIdleRejectionReason {
        self.reason
    }

    /// Consumes the rejection and returns the original model-visible input
    /// unchanged, so callers can retry, drop, or log it explicitly.
    pub fn into_input(self) -> Vec<ResponseItem> {
        self.input
    }
}

impl ThreadConfigSnapshot {
    pub fn cwd(&self) -> &AbsolutePathBuf {
        &self.environments.legacy_fallback_cwd
    }

    pub fn environment_selections(&self) -> &[TurnEnvironmentSelection] {
        &self.environments.environments
    }

    pub fn sandbox_policy(&self) -> SandboxPolicy {
        forall_sandboxing::compatibility_sandbox_policy_for_permission_profile(
            &self.permission_profile,
            self.cwd().as_path(),
        )
    }
}

/// Thread settings overrides that app-server validates before starting a turn.
#[derive(Clone, Default)]
pub struct ForallThreadSettingsOverrides {
    pub environments: Option<TurnEnvironmentSelections>,
    pub workspace_roots: Option<Vec<AbsolutePathBuf>>,
    pub profile_workspace_roots: Option<Vec<AbsolutePathBuf>>,
    pub approval_policy: Option<AskForApproval>,
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    pub sandbox_policy: Option<SandboxPolicy>,
    pub permission_profile: Option<PermissionProfile>,
    pub active_permission_profile: Option<ActivePermissionProfile>,
    pub windows_sandbox_level: Option<WindowsSandboxLevel>,
    pub model: Option<String>,
    pub effort: Option<Option<ReasoningEffort>>,
    pub summary: Option<ReasoningSummary>,
    pub service_tier: Option<Option<String>>,
    pub collaboration_mode: Option<CollaborationMode>,
    pub multi_agent_mode: Option<MultiAgentMode>,
    pub personality: Option<Personality>,
}

pub struct ForallThread {
    pub(crate) forall: Forall,
    pub(crate) session_source: SessionSource,
    session_configured: SessionConfiguredEvent,
    rollout_path: Option<PathBuf>,
    out_of_band_elicitation_count: Mutex<u64>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct BackgroundTerminalInfo {
    pub item_id: String,
    pub process_id: String,
    pub command: String,
    pub cwd: PathUri,
}

/// Conduit for the bidirectional stream of messages that compose a thread
/// (formerly called a conversation) in Forall.
impl ForallThread {
    pub(crate) fn new(
        forall: Forall,
        session_configured: SessionConfiguredEvent,
        rollout_path: Option<PathBuf>,
        session_source: SessionSource,
    ) -> Self {
        Self {
            forall,
            session_source,
            session_configured,
            rollout_path,
            out_of_band_elicitation_count: Mutex::new(0),
        }
    }

    pub async fn submit(&self, op: Op) -> ForallResult<String> {
        self.forall.submit(op).await
    }

    /// Returns the session telemetry handle for thread-scoped production instrumentation.
    pub fn session_telemetry(&self) -> SessionTelemetry {
        self.forall.session.services.session_telemetry.clone()
    }

    pub async fn shutdown_and_wait(&self) -> ForallResult<()> {
        self.forall.shutdown_and_wait().await
    }

    /// Wait until the underlying session loop has terminated.
    pub async fn wait_until_terminated(&self) {
        self.forall.session_loop_termination.clone().await;
    }

    pub(crate) async fn emit_thread_resume_lifecycle(&self) {
        for contributor in self
            .forall
            .session
            .services
            .extensions
            .thread_lifecycle_contributors()
        {
            contributor
                .on_thread_resume(forall_extension_api::ThreadResumeInput {
                    session_store: &self.forall.session.services.session_extension_data,
                    thread_store: &self.forall.session.services.thread_extension_data,
                })
                .await;
        }
    }

    pub async fn emit_thread_idle_lifecycle_if_idle(&self) {
        self.forall
            .session
            .emit_thread_idle_lifecycle_if_idle()
            .await;
    }

    #[doc(hidden)]
    pub async fn ensure_rollout_materialized(&self) {
        self.forall.session.ensure_rollout_materialized().await;
    }

    #[doc(hidden)]
    pub async fn flush_rollout(&self) -> std::io::Result<()> {
        self.forall.session.flush_rollout().await
    }

    pub async fn submit_with_trace(
        &self,
        op: Op,
        trace: Option<W3cTraceContext>,
    ) -> ForallResult<String> {
        self.forall.submit_with_trace(op, trace).await
    }

    pub async fn submit_user_input_with_client_user_message_id(
        &self,
        op: Op,
        trace: Option<W3cTraceContext>,
        client_user_message_id: Option<String>,
    ) -> ForallResult<String> {
        self.forall
            .session
            .services
            .agent_control
            .ensure_execution_capacity_for_op(self.session_configured.thread_id, &op)
            .await?;
        self.forall
            .submit_user_input_with_client_user_message_id(op, trace, client_user_message_id)
            .await
    }

    /// Persist whether this thread is eligible for future memory generation.
    pub async fn set_thread_memory_mode(&self, mode: ThreadMemoryMode) -> anyhow::Result<()> {
        self.forall.set_thread_memory_mode(mode).await
    }

    pub async fn steer_input(
        &self,
        input: Vec<UserInput>,
        additional_context: BTreeMap<String, AdditionalContextEntry>,
        expected_turn_id: Option<&str>,
        client_user_message_id: Option<String>,
        responsesapi_client_metadata: Option<HashMap<String, String>>,
    ) -> Result<String, SteerInputError> {
        self.forall
            .steer_input(
                input,
                additional_context,
                expected_turn_id,
                client_user_message_id,
                responsesapi_client_metadata,
            )
            .await
    }

    /// Injects model-visible items into the currently active turn.
    ///
    /// This is the thread-level bridge to `Session::inject_if_running` for
    /// callers that only hold a `ForallThread`.
    /// It returns the unchanged items when this thread has no active turn.
    pub async fn inject_if_running(
        &self,
        items: Vec<ResponseItem>,
    ) -> Result<(), Vec<ResponseItem>> {
        self.forall.session.inject_if_running(items).await
    }

    /// Starts an automatic regular turn with model-visible items only when idle
    /// work is allowed for this thread.
    ///
    /// This is the required entry point for extensions that want to launch
    /// model-visible work from `ThreadLifecycleContributor::on_thread_idle`.
    /// The call succeeds only if no user/client-triggered turn is queued, no
    /// task is currently active, and the thread is not in Plan mode. Active
    /// Review tasks are rejected by the active-task check because Review turns
    /// are not steerable.
    ///
    /// On rejection, the returned error includes a stable reason and carries
    /// the original `items` unchanged so the caller can decide whether to drop
    /// them, retry later, or log why no automatic turn was started.
    pub async fn try_start_turn_if_idle(
        &self,
        items: Vec<ResponseItem>,
    ) -> Result<(), TryStartTurnIfIdleError> {
        self.forall.session.try_start_turn_if_idle(items).await
    }

    pub async fn set_app_server_client_info(
        &self,
        app_server_client_name: Option<String>,
        app_server_client_version: Option<String>,
        mcp_elicitations_auto_deny: bool,
    ) -> ConstraintResult<()> {
        self.forall
            .set_app_server_client_info(
                app_server_client_name,
                app_server_client_version,
                mcp_elicitations_auto_deny,
            )
            .await
    }

    pub async fn set_openai_form_elicitation_support(&self, supported: bool) -> anyhow::Result<()> {
        self.forall
            .session
            .set_openai_form_elicitation_support(supported)
            .await
    }

    /// Preview persistent thread settings overrides without committing them.
    pub async fn preview_thread_settings_overrides(
        &self,
        overrides: ForallThreadSettingsOverrides,
    ) -> ConstraintResult<ThreadConfigSnapshot> {
        let updates = self.thread_settings_update(overrides).await;
        self.forall.session.preview_settings(&updates).await
    }

    async fn thread_settings_update(
        &self,
        overrides: ForallThreadSettingsOverrides,
    ) -> SessionSettingsUpdate {
        let ForallThreadSettingsOverrides {
            environments,
            workspace_roots,
            profile_workspace_roots,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            permission_profile,
            active_permission_profile,
            windows_sandbox_level,
            model,
            effort,
            summary,
            service_tier,
            collaboration_mode,
            multi_agent_mode,
            personality: _,
        } = overrides;
        let collaboration_mode = if let Some(collaboration_mode) = collaboration_mode {
            collaboration_mode
        } else {
            self.forall
                .session
                .collaboration_mode()
                .await
                .with_updates(model, effort, /*developer_instructions*/ None)
        };

        SessionSettingsUpdate {
            environments,
            workspace_roots,
            profile_workspace_roots,
            approval_policy,
            approvals_reviewer,
            sandbox_policy,
            permission_profile,
            active_permission_profile,
            windows_sandbox_level,
            collaboration_mode: Some(collaboration_mode),
            multi_agent_mode,
            reasoning_summary: summary,
            service_tier,
            ..Default::default()
        }
    }

    /// Use sparingly: this is intended to be removed soon.
    pub async fn submit_with_id(&self, sub: Submission) -> ForallResult<()> {
        self.forall.submit_with_id(sub).await
    }

    pub async fn next_event(&self) -> ForallResult<Event> {
        self.forall.next_event().await
    }

    pub async fn agent_status(&self) -> AgentStatus {
        self.forall.agent_status().await
    }

    pub async fn list_background_terminals(&self) -> Vec<BackgroundTerminalInfo> {
        self.forall.session.list_background_terminals().await
    }

    pub async fn terminate_background_terminal(&self, process_id: i32) -> bool {
        self.forall
            .session
            .terminate_background_terminal(process_id)
            .await
    }

    pub(crate) fn subscribe_status(&self) -> watch::Receiver<AgentStatus> {
        self.forall.agent_status.clone()
    }

    /// Returns the complete token usage snapshot currently cached for this thread.
    ///
    /// This accessor is intentionally narrower than direct session access: it lets
    /// app-server lifecycle paths replay restored usage after resume or fork without
    /// exposing broader session mutation authority. A caller that only reads
    /// `total_token_usage` would drop last-turn usage and make the v2
    /// `thread/tokenUsage/updated` payload incomplete.
    pub async fn token_usage_info(&self) -> Option<TokenUsageInfo> {
        self.forall.session.token_usage_info().await
    }

    /// Records a user-role session-prefix message without creating a new user turn boundary.
    pub(crate) async fn inject_user_message_without_turn(&self, message: String) {
        let item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText { text: message }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        };
        self.forall
            .session
            .inject_no_new_turn(vec![item], /*current_turn_context*/ None)
            .await;
    }

    /// Record raw Responses API items without starting a new turn.
    pub async fn inject_response_items(&self, items: Vec<ResponseItem>) -> ForallResult<()> {
        if items.is_empty() {
            return Err(ForallErr::InvalidRequest(
                "items must not be empty".to_string(),
            ));
        }

        let turn_context = self.forall.session.new_default_turn().await;
        if self.forall.session.reference_context_item().await.is_none() {
            self.forall
                .session
                .record_context_updates_and_set_reference_context_item(turn_context.as_ref())
                .await;
        }
        self.forall
            .session
            .inject_no_new_turn(items, Some(turn_context.as_ref()))
            .await;
        self.forall.session.flush_rollout().await?;
        Ok(())
    }

    pub fn rollout_path(&self) -> Option<PathBuf> {
        self.rollout_path.clone()
    }

    pub fn session_configured(&self) -> SessionConfiguredEvent {
        self.session_configured.clone()
    }

    pub(crate) fn is_running(&self) -> bool {
        !self.forall.tx_sub.is_closed()
    }

    pub async fn guardian_trunk_rollout_path(&self) -> Option<PathBuf> {
        self.forall
            .session
            .guardian_review_session
            .trunk_rollout_path()
            .await
    }

    pub async fn load_history(
        &self,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThreadHistory> {
        let live_thread = self
            .forall
            .session
            .live_thread_for_persistence("load history")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.load_history(include_archived).await
    }

    pub async fn read_thread(
        &self,
        include_archived: bool,
        include_history: bool,
    ) -> ThreadStoreResult<StoredThread> {
        let live_thread = self
            .forall
            .session
            .live_thread_for_persistence("read thread")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread
            .read_thread(include_archived, include_history)
            .await
    }

    pub async fn update_thread_metadata(
        &self,
        patch: ThreadMetadataPatch,
        include_archived: bool,
    ) -> ThreadStoreResult<StoredThread> {
        let live_thread = self
            .forall
            .session
            .live_thread_for_persistence("update thread metadata")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.update_metadata(patch, include_archived).await
    }

    /// Appends rollout items through the live thread so derived metadata stays in sync.
    pub async fn append_rollout_items(&self, items: &[RolloutItem]) -> ThreadStoreResult<()> {
        let live_thread = self
            .forall
            .session
            .live_thread_for_persistence("append rollout items")
            .map_err(|err| ThreadStoreError::Internal {
                message: err.to_string(),
            })?;
        live_thread.append_items(items).await
    }

    pub fn state_db(&self) -> Option<StateDbHandle> {
        self.forall.state_db()
    }

    pub async fn config_snapshot(&self) -> ThreadConfigSnapshot {
        self.forall.thread_config_snapshot().await
    }

    /// Returns the files that supplied the thread's loaded model instructions.
    pub async fn instruction_sources(&self) -> Vec<PathUri> {
        self.forall.instruction_sources().await
    }

    /// Returns loaded instruction sources rendered as legacy app-server path strings.
    pub async fn legacy_instruction_sources(&self) -> Vec<LegacyAppPathString> {
        self.instruction_sources()
            .await
            .into_iter()
            .map(Into::into)
            .collect()
    }

    pub async fn config(&self) -> Arc<crate::config::Config> {
        self.forall.session.get_config().await
    }

    /// Resolves the MCP runtime configuration using this thread's extension data.
    pub async fn runtime_mcp_config(
        &self,
        config: &crate::config::Config,
    ) -> forall_mcp::McpConfig {
        self.forall.session.runtime_mcp_config(config).await
    }

    pub fn multi_agent_version(&self) -> Option<MultiAgentVersion> {
        self.forall.session.multi_agent_version()
    }

    /// Refresh the thread's layer-backed user config state from a caller-supplied
    /// config snapshot. Thread-scoped layers and session-static settings remain
    /// unchanged.
    pub async fn refresh_runtime_config(&self, next_config: crate::config::Config) {
        self.forall
            .session
            .refresh_runtime_config(next_config)
            .await;
    }

    pub async fn environment_selections(&self) -> Vec<TurnEnvironmentSelection> {
        self.forall.thread_environment_selections().await
    }

    pub async fn read_mcp_resource(
        &self,
        server: &str,
        uri: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let result = self
            .forall
            .session
            .read_resource(server, ReadResourceRequestParams::new(uri))
            .await?;

        Ok(serde_json::to_value(result)?)
    }

    pub async fn call_mcp_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
    ) -> anyhow::Result<CallToolResult> {
        self.forall
            .session
            .call_tool(server, tool, arguments, meta)
            .await
    }

    pub fn enabled(&self, feature: Feature) -> bool {
        self.forall.enabled(feature)
    }

    pub async fn increment_out_of_band_elicitation_count(&self) -> ForallResult<u64> {
        let mut guard = self.out_of_band_elicitation_count.lock().await;
        let was_zero = *guard == 0;
        *guard = guard.checked_add(1).ok_or_else(|| {
            ForallErr::Fatal("out-of-band elicitation count overflowed".to_string())
        })?;

        if was_zero {
            self.forall
                .session
                .set_out_of_band_elicitation_pause_state(/*paused*/ true);
        }

        Ok(*guard)
    }

    pub async fn decrement_out_of_band_elicitation_count(&self) -> ForallResult<u64> {
        let mut guard = self.out_of_band_elicitation_count.lock().await;
        if *guard == 0 {
            return Err(ForallErr::InvalidRequest(
                "out-of-band elicitation count is already zero".to_string(),
            ));
        }

        *guard -= 1;
        let now_zero = *guard == 0;
        if now_zero {
            self.forall
                .session
                .set_out_of_band_elicitation_pause_state(/*paused*/ false);
        }

        Ok(*guard)
    }
}
