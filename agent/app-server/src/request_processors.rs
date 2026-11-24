use crate::bespoke_event_handling::apply_bespoke_event_handling;
use crate::bespoke_event_handling::maybe_emit_hook_prompt_item_completed;
use crate::command_exec::CommandExecManager;
use crate::command_exec::StartCommandExecParams;
use crate::config_manager::ConfigManager;
use crate::error_code::INPUT_TOO_LARGE_ERROR_CODE;
use crate::error_code::invalid_params;
use crate::models::supported_models;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use crate::outgoing_message::OutgoingMessageSender;
use crate::outgoing_message::RequestContext;
use crate::outgoing_message::ThreadScopedOutgoingMessageSender;
use crate::skills_watcher::SkillsWatcher;
use crate::thread_status::ThreadWatchManager;
use crate::thread_status::resolve_thread_status;
use chrono::Duration as ChronoDuration;
use chrono::SecondsFormat;
use forall_analytics::AnalyticsEventsClient;
use forall_analytics::AnalyticsJsonRpcError;
use forall_analytics::InputError;
use forall_analytics::TurnSteerRequestError;
use forall_app_server_protocol::Account;
use forall_app_server_protocol::AccountLoginCompletedNotification;
use forall_app_server_protocol::AccountUpdatedNotification;
use forall_app_server_protocol::AdditionalContextEntry;
use forall_app_server_protocol::AdditionalContextKind;
use forall_app_server_protocol::AskForApproval;
use forall_app_server_protocol::AuthMode;
use forall_app_server_protocol::CancelLoginAccountParams;
use forall_app_server_protocol::CancelLoginAccountResponse;
use forall_app_server_protocol::CancelLoginAccountStatus;
use forall_app_server_protocol::ClientInfo;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::ClientResponsePayload;
use forall_app_server_protocol::CollaborationModeListParams;
use forall_app_server_protocol::CollaborationModeListResponse;
use forall_app_server_protocol::CommandExecParams;
use forall_app_server_protocol::CommandExecResizeParams;
use forall_app_server_protocol::CommandExecTerminateParams;
use forall_app_server_protocol::CommandExecWriteParams;
use forall_app_server_protocol::ConfigWarningNotification;
use forall_app_server_protocol::ConsumeAccountRateLimitResetCreditParams;
use forall_app_server_protocol::ConversationGitInfo;
use forall_app_server_protocol::ConversationSummary;
use forall_app_server_protocol::DynamicToolFunctionSpec;
use forall_app_server_protocol::DynamicToolNamespaceTool;
use forall_app_server_protocol::DynamicToolSpec;
use forall_app_server_protocol::EnvironmentAddParams;
use forall_app_server_protocol::EnvironmentAddResponse;
use forall_app_server_protocol::ExperimentalFeature as ApiExperimentalFeature;
use forall_app_server_protocol::ExperimentalFeatureListParams;
use forall_app_server_protocol::ExperimentalFeatureListResponse;
use forall_app_server_protocol::ExperimentalFeatureStage as ApiExperimentalFeatureStage;
use forall_app_server_protocol::ForallErrorInfo;
use forall_app_server_protocol::GetAccountParams;
use forall_app_server_protocol::GetAccountRateLimitsResponse;
use forall_app_server_protocol::GetAccountResponse;
use forall_app_server_protocol::GetAccountTokenUsageResponse;
use forall_app_server_protocol::GetAuthStatusParams;
use forall_app_server_protocol::GetAuthStatusResponse;
use forall_app_server_protocol::GetConversationSummaryParams;
use forall_app_server_protocol::GetConversationSummaryResponse;
use forall_app_server_protocol::GetWorkspaceMessagesResponse;
use forall_app_server_protocol::GitDiffToRemoteParams;
use forall_app_server_protocol::GitDiffToRemoteResponse;
use forall_app_server_protocol::GitInfo as ApiGitInfo;
use forall_app_server_protocol::GithubAccountReadResponse;
use forall_app_server_protocol::GithubConnectCancelParams;
use forall_app_server_protocol::GithubConnectCancelResponse;
use forall_app_server_protocol::GithubConnectCancelStatus;
use forall_app_server_protocol::GithubConnectCompletedNotification;
use forall_app_server_protocol::GithubConnectStartResponse;
use forall_app_server_protocol::GithubDisconnectResponse;
use forall_app_server_protocol::GithubRepositoriesListResponse;
use forall_app_server_protocol::GithubRepository;
use forall_app_server_protocol::HookMetadata;
use forall_app_server_protocol::HooksListParams;
use forall_app_server_protocol::HooksListResponse;
use forall_app_server_protocol::InitializeParams;
use forall_app_server_protocol::InitializeResponse;
use forall_app_server_protocol::JSONRPCErrorError;
use forall_app_server_protocol::ListMcpServerStatusParams;
use forall_app_server_protocol::ListMcpServerStatusResponse;
use forall_app_server_protocol::LoginAccountParams;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::LoginApiKeyParams;
use forall_app_server_protocol::LogoutAccountResponse;
use forall_app_server_protocol::MarketplaceAddParams;
use forall_app_server_protocol::MarketplaceAddResponse;
use forall_app_server_protocol::MarketplaceInterface;
use forall_app_server_protocol::MarketplaceRemoveParams;
use forall_app_server_protocol::MarketplaceRemoveResponse;
use forall_app_server_protocol::MarketplaceUpgradeErrorInfo;
use forall_app_server_protocol::MarketplaceUpgradeParams;
use forall_app_server_protocol::MarketplaceUpgradeResponse;
use forall_app_server_protocol::McpResourceReadParams;
use forall_app_server_protocol::McpResourceReadResponse;
use forall_app_server_protocol::McpServerOauthLoginCompletedNotification;
use forall_app_server_protocol::McpServerOauthLoginParams;
use forall_app_server_protocol::McpServerOauthLoginResponse;
use forall_app_server_protocol::McpServerRefreshResponse;
use forall_app_server_protocol::McpServerStatus;
use forall_app_server_protocol::McpServerStatusDetail;
use forall_app_server_protocol::McpServerToolCallParams;
use forall_app_server_protocol::McpServerToolCallResponse;
use forall_app_server_protocol::MemoryResetResponse;
use forall_app_server_protocol::MockExperimentalMethodParams;
use forall_app_server_protocol::MockExperimentalMethodResponse;
use forall_app_server_protocol::ModelListParams;
use forall_app_server_protocol::ModelListResponse;
use forall_app_server_protocol::OauthSignInProvider;
use forall_app_server_protocol::PermissionProfileListParams;
use forall_app_server_protocol::PermissionProfileListResponse;
use forall_app_server_protocol::PermissionProfileSummary;
use forall_app_server_protocol::PluginDetail;
use forall_app_server_protocol::PluginInstallParams;
use forall_app_server_protocol::PluginInstallResponse;
use forall_app_server_protocol::PluginInstalledParams;
use forall_app_server_protocol::PluginInstalledResponse;
use forall_app_server_protocol::PluginInterface;
use forall_app_server_protocol::PluginListMarketplaceKind;
use forall_app_server_protocol::PluginListParams;
use forall_app_server_protocol::PluginListResponse;
use forall_app_server_protocol::PluginMarketplaceEntry;
use forall_app_server_protocol::PluginReadParams;
use forall_app_server_protocol::PluginReadResponse;
use forall_app_server_protocol::PluginShareCheckoutParams;
use forall_app_server_protocol::PluginShareCheckoutResponse;
use forall_app_server_protocol::PluginShareContext;
use forall_app_server_protocol::PluginShareDeleteParams;
use forall_app_server_protocol::PluginShareDeleteResponse;
use forall_app_server_protocol::PluginShareDiscoverability;
use forall_app_server_protocol::PluginShareListItem;
use forall_app_server_protocol::PluginShareListParams;
use forall_app_server_protocol::PluginShareListResponse;
use forall_app_server_protocol::PluginSharePrincipal;
use forall_app_server_protocol::PluginSharePrincipalType;
use forall_app_server_protocol::PluginShareSaveParams;
use forall_app_server_protocol::PluginShareSaveResponse;
use forall_app_server_protocol::PluginShareTarget;
use forall_app_server_protocol::PluginShareUpdateDiscoverability;
use forall_app_server_protocol::PluginShareUpdateTargetsParams;
use forall_app_server_protocol::PluginShareUpdateTargetsResponse;
use forall_app_server_protocol::PluginSkillReadParams;
use forall_app_server_protocol::PluginSkillReadResponse;
use forall_app_server_protocol::PluginSource;
use forall_app_server_protocol::PluginSummary;
use forall_app_server_protocol::PluginUninstallParams;
use forall_app_server_protocol::PluginUninstallResponse;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::ReviewDelivery as ApiReviewDelivery;
use forall_app_server_protocol::ReviewStartParams;
use forall_app_server_protocol::ReviewStartResponse;
use forall_app_server_protocol::ReviewTarget as ApiReviewTarget;
use forall_app_server_protocol::SandboxMode;
use forall_app_server_protocol::SendAddCreditsNudgeEmailParams;
use forall_app_server_protocol::SendAddCreditsNudgeEmailResponse;
use forall_app_server_protocol::ServerNotification;
use forall_app_server_protocol::ServerRequestResolvedNotification;
use forall_app_server_protocol::SkillSummary;
use forall_app_server_protocol::SkillsConfigWriteParams;
use forall_app_server_protocol::SkillsConfigWriteResponse;
use forall_app_server_protocol::SkillsExtraRootsSetParams;
use forall_app_server_protocol::SkillsExtraRootsSetResponse;
use forall_app_server_protocol::SkillsListParams;
use forall_app_server_protocol::SkillsListResponse;
use forall_app_server_protocol::SortDirection;
use forall_app_server_protocol::Thread;
use forall_app_server_protocol::ThreadApproveGuardianDeniedActionParams;
use forall_app_server_protocol::ThreadApproveGuardianDeniedActionResponse;
use forall_app_server_protocol::ThreadArchiveParams;
use forall_app_server_protocol::ThreadArchiveResponse;
use forall_app_server_protocol::ThreadArchivedNotification;
use forall_app_server_protocol::ThreadBackgroundTerminal;
use forall_app_server_protocol::ThreadBackgroundTerminalsCleanParams;
use forall_app_server_protocol::ThreadBackgroundTerminalsCleanResponse;
use forall_app_server_protocol::ThreadBackgroundTerminalsListParams;
use forall_app_server_protocol::ThreadBackgroundTerminalsListResponse;
use forall_app_server_protocol::ThreadBackgroundTerminalsTerminateParams;
use forall_app_server_protocol::ThreadBackgroundTerminalsTerminateResponse;
use forall_app_server_protocol::ThreadClosedNotification;
use forall_app_server_protocol::ThreadCompactStartParams;
use forall_app_server_protocol::ThreadCompactStartResponse;
use forall_app_server_protocol::ThreadDecrementElicitationParams;
use forall_app_server_protocol::ThreadDecrementElicitationResponse;
use forall_app_server_protocol::ThreadDeleteParams;
use forall_app_server_protocol::ThreadDeleteResponse;
use forall_app_server_protocol::ThreadDeletedNotification;
use forall_app_server_protocol::ThreadForkParams;
use forall_app_server_protocol::ThreadForkResponse;
use forall_app_server_protocol::ThreadGoal;
use forall_app_server_protocol::ThreadGoalClearParams;
use forall_app_server_protocol::ThreadGoalClearResponse;
use forall_app_server_protocol::ThreadGoalClearedNotification;
use forall_app_server_protocol::ThreadGoalGetParams;
use forall_app_server_protocol::ThreadGoalGetResponse;
use forall_app_server_protocol::ThreadGoalSetParams;
use forall_app_server_protocol::ThreadGoalSetResponse;
use forall_app_server_protocol::ThreadGoalStatus;
use forall_app_server_protocol::ThreadGoalUpdatedNotification;
use forall_app_server_protocol::ThreadHistoryBuilder;
use forall_app_server_protocol::ThreadIncrementElicitationParams;
use forall_app_server_protocol::ThreadIncrementElicitationResponse;
use forall_app_server_protocol::ThreadInjectItemsParams;
use forall_app_server_protocol::ThreadInjectItemsResponse;
use forall_app_server_protocol::ThreadItem;
use forall_app_server_protocol::ThreadListCwdFilter;
use forall_app_server_protocol::ThreadListParams;
use forall_app_server_protocol::ThreadListResponse;
use forall_app_server_protocol::ThreadLoadedListParams;
use forall_app_server_protocol::ThreadLoadedListResponse;
use forall_app_server_protocol::ThreadMemoryModeSetParams;
use forall_app_server_protocol::ThreadMemoryModeSetResponse;
use forall_app_server_protocol::ThreadMetadataGitInfoUpdateParams;
use forall_app_server_protocol::ThreadMetadataUpdateParams;
use forall_app_server_protocol::ThreadMetadataUpdateResponse;
use forall_app_server_protocol::ThreadNameUpdatedNotification;
use forall_app_server_protocol::ThreadReadParams;
use forall_app_server_protocol::ThreadReadResponse;
use forall_app_server_protocol::ThreadRealtimeAppendAudioParams;
use forall_app_server_protocol::ThreadRealtimeAppendAudioResponse;
use forall_app_server_protocol::ThreadRealtimeAppendSpeechParams;
use forall_app_server_protocol::ThreadRealtimeAppendSpeechResponse;
use forall_app_server_protocol::ThreadRealtimeAppendTextParams;
use forall_app_server_protocol::ThreadRealtimeAppendTextResponse;
use forall_app_server_protocol::ThreadRealtimeListVoicesResponse;
use forall_app_server_protocol::ThreadRealtimeStartParams;
use forall_app_server_protocol::ThreadRealtimeStartResponse;
use forall_app_server_protocol::ThreadRealtimeStartTransport;
use forall_app_server_protocol::ThreadRealtimeStopParams;
use forall_app_server_protocol::ThreadRealtimeStopResponse;
use forall_app_server_protocol::ThreadResumeInitialTurnsPageParams;
use forall_app_server_protocol::ThreadResumeParams;
use forall_app_server_protocol::ThreadResumeResponse;
use forall_app_server_protocol::ThreadRollbackParams;
use forall_app_server_protocol::ThreadSearchParams;
use forall_app_server_protocol::ThreadSearchResponse;
use forall_app_server_protocol::ThreadSearchResult;
use forall_app_server_protocol::ThreadSetNameParams;
use forall_app_server_protocol::ThreadSetNameResponse;
use forall_app_server_protocol::ThreadSettings;
use forall_app_server_protocol::ThreadSettingsUpdateParams;
use forall_app_server_protocol::ThreadSettingsUpdateResponse;
use forall_app_server_protocol::ThreadShellCommandParams;
use forall_app_server_protocol::ThreadShellCommandResponse;
use forall_app_server_protocol::ThreadSortKey;
use forall_app_server_protocol::ThreadSourceKind;
use forall_app_server_protocol::ThreadStartParams;
use forall_app_server_protocol::ThreadStartResponse;
use forall_app_server_protocol::ThreadStartedNotification;
use forall_app_server_protocol::ThreadStatus;
use forall_app_server_protocol::ThreadTurnsItemsListParams;
use forall_app_server_protocol::ThreadTurnsListParams;
use forall_app_server_protocol::ThreadTurnsListResponse;
use forall_app_server_protocol::ThreadUnarchiveParams;
use forall_app_server_protocol::ThreadUnarchiveResponse;
use forall_app_server_protocol::ThreadUnarchivedNotification;
use forall_app_server_protocol::ThreadUnsubscribeParams;
use forall_app_server_protocol::ThreadUnsubscribeResponse;
use forall_app_server_protocol::ThreadUnsubscribeStatus;
use forall_app_server_protocol::Turn;
use forall_app_server_protocol::TurnEnvironmentParams;
use forall_app_server_protocol::TurnError;
use forall_app_server_protocol::TurnInterruptParams;
use forall_app_server_protocol::TurnInterruptResponse;
use forall_app_server_protocol::TurnItemsView;
use forall_app_server_protocol::TurnStartParams;
use forall_app_server_protocol::TurnStartResponse;
use forall_app_server_protocol::TurnStatus;
use forall_app_server_protocol::TurnSteerParams;
use forall_app_server_protocol::TurnSteerResponse;
use forall_app_server_protocol::UserInput as V2UserInput;
use forall_app_server_protocol::WindowsSandboxReadiness;
use forall_app_server_protocol::WindowsSandboxReadinessResponse;
use forall_app_server_protocol::WindowsSandboxSetupCompletedNotification;
use forall_app_server_protocol::WindowsSandboxSetupMode;
use forall_app_server_protocol::WindowsSandboxSetupStartParams;
use forall_app_server_protocol::WindowsSandboxSetupStartResponse;
use forall_arg0::Arg0DispatchPaths;
use forall_config::CloudConfigBundleLoadError;
use forall_config::CloudConfigBundleLoadErrorCode;
use forall_config::ConfigLayerStack;
use forall_config::loader::project_trust_key;
use forall_config::types::McpServerTransportConfig;
use forall_core::ForallThread;
use forall_core::ForallThreadSettingsOverrides;
use forall_core::ForkSnapshot;
use forall_core::NewThread;
#[cfg(test)]
use forall_core::SessionMeta;
use forall_core::StartThreadOptions;
use forall_core::SteerInputError;
use forall_core::ThreadConfigSnapshot;
use forall_core::ThreadManager;
use forall_core::config::Config;
use forall_core::config::ConfigOverrides;
use forall_core::config::NetworkProxyAuditMetadata;
use forall_core::config::edit::ConfigEdit;
use forall_core::config::edit::ConfigEditsBuilder;
use forall_core::exec::ExecCapturePolicy;
use forall_core::exec::ExecExpiration;
use forall_core::exec::ExecParams;
use forall_core::exec_env::create_env;
use forall_core::path_utils;
#[cfg(test)]
use forall_core::read_head_for_summary;
use forall_core::sandboxing::SandboxPermissions;
use forall_core::windows_sandbox::WindowsSandboxLevelExt;
use forall_core::windows_sandbox::WindowsSandboxSetupMode as CoreWindowsSandboxSetupMode;
use forall_core::windows_sandbox::WindowsSandboxSetupRequest;
use forall_core::windows_sandbox::sandbox_setup_is_complete;
use forall_core::workspace_settings;
use forall_core_plugins::PluginInstallError as CorePluginInstallError;
use forall_core_plugins::PluginInstallRequest;
use forall_core_plugins::PluginReadRequest;
use forall_core_plugins::PluginUninstallError as CorePluginUninstallError;
use forall_core_plugins::loader::load_plugin_mcp_servers;
use forall_core_plugins::manifest::PluginManifestInterface;
use forall_core_plugins::marketplace::MarketplaceError;
use forall_core_plugins::marketplace::MarketplacePluginSource;
use forall_core_plugins::marketplace_add::MarketplaceAddError;
use forall_core_plugins::marketplace_add::MarketplaceAddRequest;
use forall_core_plugins::marketplace_add::add_marketplace as add_marketplace_to_forall_home;
use forall_core_plugins::marketplace_remove::MarketplaceRemoveError;
use forall_core_plugins::marketplace_remove::MarketplaceRemoveRequest as CoreMarketplaceRemoveRequest;
use forall_core_plugins::marketplace_remove::remove_marketplace;
use forall_core_plugins::remote::RemoteMarketplace;
use forall_core_plugins::remote::RemoteMarketplaceSource;
use forall_core_plugins::remote::RemotePluginCatalogError;
use forall_core_plugins::remote::RemotePluginDetail as RemoteCatalogPluginDetail;
use forall_core_plugins::remote::RemotePluginServiceConfig;
use forall_core_plugins::remote::RemotePluginShareContext as RemoteCatalogPluginShareContext;
use forall_core_plugins::remote::RemotePluginShareSummary as RemoteCatalogPluginShareSummary;
use forall_core_plugins::remote::RemotePluginSummary as RemoteCatalogPluginSummary;
use forall_exec_server::EnvironmentManager;
use forall_exec_server::LOCAL_ENVIRONMENT_ID;
use forall_exec_server::LOCAL_FS;
use forall_features::FEATURES;
use forall_features::Feature;
use forall_features::Stage;
use forall_git_utils::git_diff_to_remote;
use forall_git_utils::resolve_root_git_project_for_trust;
use forall_login::AuthBaseUrls;
use forall_login::AuthManager;
use forall_login::CancelHandle;
use forall_login::ForallAuth;
use forall_login::GITHUB_API_BASE;
use forall_login::GithubConnectFlow;
use forall_login::GithubConnectOptions;
use forall_login::OauthLoginFlow;
use forall_login::OauthLoginOptions;
use forall_login::OauthLoginProvider;
use forall_login::disconnect_github;
use forall_login::list_repositories;
use forall_login::load_github_connection;
use forall_login::set_byok_api_key;
use forall_mcp::McpRuntimeContext;
use forall_mcp::McpServerStatusSnapshot;
use forall_mcp::McpSnapshotDetail;
use forall_mcp::collect_mcp_server_status_snapshot_with_detail;
use forall_mcp::discover_supported_scopes;
use forall_mcp::read_mcp_resource as read_mcp_resource_without_thread;
use forall_mcp::resolve_oauth_scopes;
use forall_memories_write::clear_memory_roots_contents;
use forall_model_provider::create_model_provider;
use forall_models_manager::collaboration_mode_presets::builtin_collaboration_mode_presets;
use forall_protocol::ThreadId;
use forall_protocol::config_types::CollaborationMode;
use forall_protocol::config_types::Personality;
use forall_protocol::config_types::ReasoningSummary;
use forall_protocol::config_types::TrustLevel;
use forall_protocol::config_types::WindowsSandboxLevel;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result as ForallResult;
#[cfg(test)]
use forall_protocol::items::TurnItem;
use forall_protocol::models::ResponseItem;
use forall_protocol::openai_models::ReasoningEffort;
#[cfg(test)]
use forall_protocol::permissions::FileSystemSandboxPolicy;
use forall_protocol::protocol::AgentStatus;
use forall_protocol::protocol::ConversationAudioParams;
use forall_protocol::protocol::ConversationSpeechParams;
use forall_protocol::protocol::ConversationStartParams;
use forall_protocol::protocol::ConversationStartTransport;
use forall_protocol::protocol::ConversationTextParams;
use forall_protocol::protocol::EventMsg;
#[cfg(test)]
use forall_protocol::protocol::GitInfo as CoreGitInfo;
use forall_protocol::protocol::InitialHistory;
use forall_protocol::protocol::McpAuthStatus as CoreMcpAuthStatus;
use forall_protocol::protocol::Op;
use forall_protocol::protocol::RealtimeVoicesList;
use forall_protocol::protocol::ResumedHistory;
use forall_protocol::protocol::ReviewDelivery as CoreReviewDelivery;
use forall_protocol::protocol::ReviewRequest;
use forall_protocol::protocol::ReviewTarget as CoreReviewTarget;
use forall_protocol::protocol::RolloutItem;
use forall_protocol::protocol::SessionConfiguredEvent;
#[cfg(test)]
use forall_protocol::protocol::SessionMetaLine;
use forall_protocol::protocol::TurnEnvironmentSelection;
use forall_protocol::protocol::TurnEnvironmentSelections;
use forall_protocol::protocol::USER_MESSAGE_BEGIN;
use forall_protocol::protocol::W3cTraceContext;
use forall_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use forall_protocol::user_input::UserInput as CoreInputItem;
use forall_rmcp_client::perform_oauth_login_return_url;
use forall_rollout::is_persisted_rollout_item;
use forall_rollout::state_db::StateDbHandle;
use forall_rollout::state_db::reconcile_rollout;
use forall_state::ThreadMetadata;
use forall_state::log_db::LogDbLayer;
use forall_thread_store::ArchiveThreadParams as StoreArchiveThreadParams;
use forall_thread_store::DeleteThreadParams as StoreDeleteThreadParams;
use forall_thread_store::GitInfoPatch as StoreGitInfoPatch;
use forall_thread_store::ListThreadsParams as StoreListThreadsParams;
use forall_thread_store::LocalThreadStore;
use forall_thread_store::ReadThreadByRolloutPathParams as StoreReadThreadByRolloutPathParams;
use forall_thread_store::ReadThreadParams as StoreReadThreadParams;
use forall_thread_store::SearchThreadsParams as StoreSearchThreadsParams;
use forall_thread_store::SortDirection as StoreSortDirection;
use forall_thread_store::StoredThread;
use forall_thread_store::ThreadMetadataPatch as StoreThreadMetadataPatch;
use forall_thread_store::ThreadSortKey as StoreThreadSortKey;
use forall_thread_store::ThreadStore;
use forall_thread_store::ThreadStoreError;
use forall_utils_absolute_path::AbsolutePathBuf;
use forall_utils_pty::DEFAULT_OUTPUT_BYTES_CAP;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::result::Result;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::sync::SemaphorePermit;
use tokio::sync::broadcast;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use toml::Value as TomlValue;
use tracing::Instrument;
use tracing::error;
use tracing::info;
use tracing::warn;
use uuid::Uuid;

#[cfg(test)]
use forall_app_server_protocol::ServerRequest;

mod account_processor;
mod catalog_processor;
mod command_exec_processor;
mod config_processor;
mod environment_processor;
mod external_agent_config_processor;
mod external_agent_session_import;
mod fs_processor;
mod git_processor;
mod initialize_processor;
mod marketplace_processor;
mod mcp_processor;
mod plugins;
mod process_exec_processor;
mod search;
mod thread_processor;
mod token_usage_replay;
mod turn_processor;
mod windows_sandbox_processor;

pub(crate) use account_processor::AccountRequestProcessor;
pub(crate) use catalog_processor::CatalogRequestProcessor;
pub(crate) use command_exec_processor::CommandExecRequestProcessor;
pub(crate) use config_processor::ConfigRequestProcessor;
pub(crate) use environment_processor::EnvironmentRequestProcessor;
pub(crate) use external_agent_config_processor::ExternalAgentConfigRequestProcessor;
pub(crate) use external_agent_config_processor::ExternalAgentConfigRequestProcessorArgs;
pub(crate) use fs_processor::FsRequestProcessor;
pub(crate) use git_processor::GitRequestProcessor;
pub(crate) use initialize_processor::InitializeRequestProcessor;
pub(crate) use marketplace_processor::MarketplaceRequestProcessor;
pub(crate) use mcp_processor::McpRequestProcessor;
pub(crate) use plugins::PluginRequestProcessor;
pub(crate) use process_exec_processor::ProcessExecRequestProcessor;
pub(crate) use search::SearchRequestProcessor;
pub(crate) use thread_goal_processor::ThreadGoalRequestProcessor;
pub(crate) use thread_processor::ThreadRequestProcessor;
pub(crate) use turn_processor::TurnRequestProcessor;
pub(crate) use windows_sandbox_processor::WindowsSandboxRequestProcessor;

use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use crate::filters::compute_source_filters;
use crate::filters::source_kind_matches;
use crate::thread_state::ConnectionCapabilities;
use crate::thread_state::ThreadListenerCommand;
use crate::thread_state::ThreadState;
use crate::thread_state::ThreadStateManager;
use token_usage_replay::latest_token_usage_turn_id_from_rollout_items;
use token_usage_replay::send_thread_token_usage_update_to_connection;

fn resolve_request_cwd(cwd: Option<PathBuf>) -> Result<Option<AbsolutePathBuf>, JSONRPCErrorError> {
    cwd.map(|cwd| {
        AbsolutePathBuf::relative_to_current_dir(path_utils::normalize_for_native_workdir(cwd))
            .map_err(|err| invalid_request(format!("invalid cwd: {err}")))
    })
    .transpose()
}

fn resolve_turn_environment_selections(
    thread_manager: &ThreadManager,
    environments: Option<Vec<TurnEnvironmentParams>>,
) -> Result<Option<Vec<TurnEnvironmentSelection>>, JSONRPCErrorError> {
    let Some(environments) = environments else {
        return Ok(None);
    };
    let mut selections = Vec::with_capacity(environments.len());
    for environment in environments {
        let environment_id = environment.environment_id;
        let cwd = environment
            .cwd
            .to_inferred_path_uri()
            .ok_or_else(|| {
                invalid_request(format!(
                    "invalid cwd for environment `{environment_id}`: path `{}` does not use absolute POSIX or Windows path syntax",
                    environment.cwd
                ))
            })?;
        selections.push(TurnEnvironmentSelection {
            environment_id,
            cwd,
        });
    }
    thread_manager
        .validate_environment_selections(&selections)
        .map_err(environment_selection_error)?;
    Ok(Some(selections))
}

fn resolve_runtime_workspace_roots(workspace_roots: Vec<AbsolutePathBuf>) -> Vec<AbsolutePathBuf> {
    let mut resolved_roots = Vec::new();
    for root in workspace_roots {
        if !resolved_roots.iter().any(|existing| existing == &root) {
            resolved_roots.push(root);
        }
    }
    resolved_roots
}

mod config_errors;
mod request_errors;
mod thread_delete;
mod thread_goal_processor;
mod thread_lifecycle;
mod thread_resume_redaction;
mod thread_summary;

use self::config_errors::*;
use self::request_errors::*;
use self::thread_goal_processor::api_thread_goal_from_state;
use self::thread_lifecycle::*;
use self::thread_resume_redaction::*;
use self::thread_summary::*;

pub(crate) use self::thread_lifecycle::populate_thread_turns_from_history;
pub(crate) use self::thread_processor::thread_from_stored_thread;
#[cfg(test)]
pub(crate) use self::thread_summary::read_summary_from_rollout;
#[cfg(test)]
pub(crate) use self::thread_summary::summary_to_thread;
pub(crate) use self::thread_summary::thread_settings_from_config_snapshot;
pub(crate) use self::thread_summary::thread_settings_from_core_snapshot;

pub(crate) fn build_api_turns_from_rollout_items(items: &[RolloutItem]) -> Vec<Turn> {
    let mut builder = ThreadHistoryBuilder::new();
    for item in items {
        if is_persisted_rollout_item(item) {
            builder.handle_rollout_item(item);
        }
    }
    builder.finish()
}
