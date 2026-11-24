//! Test-only helpers exposed for cross-crate integration tests.
//!
//! Production code should not depend on this module.
//! We prefer this to using a crate feature to avoid building multiple
//! permutations of the crate.

use std::path::PathBuf;
use std::sync::Arc;

use forall_exec_server::EnvironmentManager;
use forall_extension_api::LoadUserInstructionsFuture;
use forall_extension_api::LoadedUserInstructions;
use forall_extension_api::UserInstructionsProvider;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_model_provider::create_model_provider;
use forall_model_provider_info::ModelProviderInfo;
use forall_models_manager::bundled_models_response;
use forall_models_manager::collaboration_mode_presets;
use forall_models_manager::manager::SharedModelsManager;
use forall_models_manager::test_support::construct_model_info_offline_for_tests;
use forall_models_manager::test_support::get_model_offline_for_tests;
use forall_protocol::ThreadId;
use forall_protocol::config_types::CollaborationModeMask;
use forall_protocol::openai_models::ModelInfo;
use forall_protocol::openai_models::ModelPreset;
use forall_protocol::protocol::SessionSource;
use once_cell::sync::Lazy;

use crate::ThreadManager;
use crate::config::Config;
use crate::responses_metadata::ForallResponsesMetadata;
use crate::responses_metadata::ForallResponsesRequestKind;
use crate::responses_metadata::subagent_header_value;
use crate::responses_metadata::subagent_metadata_kind;
use crate::thread_manager;
use crate::unified_exec;

static TEST_MODEL_PRESETS: Lazy<Vec<ModelPreset>> = Lazy::new(|| {
    let mut response = bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));
    response.models.sort_by_key(|model| model.priority);
    let mut presets: Vec<ModelPreset> = response.models.into_iter().map(Into::into).collect();
    ModelPreset::mark_default_by_picker_visibility(&mut presets);
    presets
});

/// Test-only provider that supplies no user instructions.
#[derive(Debug, Default)]
pub struct EmptyUserInstructionsProvider;

impl UserInstructionsProvider for EmptyUserInstructionsProvider {
    fn load_user_instructions(&self) -> LoadUserInstructionsFuture<'_> {
        Box::pin(async { LoadedUserInstructions::default() })
    }
}

pub fn set_thread_manager_test_mode(enabled: bool) {
    thread_manager::set_thread_manager_test_mode_for_tests(enabled);
}

pub fn set_deterministic_process_ids(enabled: bool) {
    unified_exec::set_deterministic_process_ids_for_tests(enabled);
}

pub fn auth_manager_from_auth(auth: ForallAuth) -> Arc<AuthManager> {
    AuthManager::from_auth_for_testing(auth)
}

pub fn auth_manager_from_auth_with_home(
    auth: ForallAuth,
    forall_home: PathBuf,
) -> Arc<AuthManager> {
    AuthManager::from_auth_for_testing_with_home(auth, forall_home)
}

pub fn thread_manager_with_models_provider(
    auth: ForallAuth,
    provider: ModelProviderInfo,
) -> ThreadManager {
    ThreadManager::with_models_provider_for_tests(auth, provider)
}

pub fn thread_manager_with_models_provider_and_home(
    auth: ForallAuth,
    provider: ModelProviderInfo,
    forall_home: PathBuf,
    environment_manager: Arc<EnvironmentManager>,
) -> ThreadManager {
    ThreadManager::with_models_provider_and_home_for_tests(
        auth,
        provider,
        forall_home,
        environment_manager,
    )
}

pub fn thread_manager_with_models_provider_home_and_state(
    auth: ForallAuth,
    provider: ModelProviderInfo,
    forall_home: PathBuf,
    environment_manager: Arc<EnvironmentManager>,
    state_db: Option<crate::StateDbHandle>,
) -> ThreadManager {
    ThreadManager::with_models_provider_home_and_state_for_tests(
        auth,
        provider,
        forall_home,
        environment_manager,
        state_db,
    )
}

pub async fn start_thread_with_user_shell_override(
    thread_manager: &ThreadManager,
    config: Config,
    user_shell_override: crate::shell::Shell,
    supports_openai_form_elicitation: bool,
) -> forall_protocol::error::Result<crate::NewThread> {
    thread_manager
        .start_thread_with_user_shell_override_for_tests(
            config,
            user_shell_override,
            supports_openai_form_elicitation,
        )
        .await
}

pub async fn resume_thread_from_rollout_with_user_shell_override(
    thread_manager: &ThreadManager,
    config: Config,
    rollout_path: PathBuf,
    auth_manager: Arc<AuthManager>,
    user_shell_override: crate::shell::Shell,
    supports_openai_form_elicitation: bool,
) -> forall_protocol::error::Result<crate::NewThread> {
    thread_manager
        .resume_thread_from_rollout_with_user_shell_override_for_tests(
            config,
            rollout_path,
            auth_manager,
            user_shell_override,
            supports_openai_form_elicitation,
        )
        .await
}

pub fn models_manager_with_provider(
    forall_home: PathBuf,
    auth_manager: Arc<AuthManager>,
    provider: ModelProviderInfo,
) -> SharedModelsManager {
    let provider = create_model_provider(provider, Some(auth_manager));
    provider.models_manager(forall_home, /*config_model_catalog*/ None)
}

pub fn get_model_offline(model: Option<&str>) -> String {
    get_model_offline_for_tests(model)
}

pub fn construct_model_info_offline(model: &str, config: &Config) -> ModelInfo {
    construct_model_info_offline_for_tests(model, &config.to_models_manager_config())
}

#[derive(Clone, Copy)]
pub enum TestForallResponsesRequestKind {
    Turn,
    Prewarm,
    WebsocketConnection,
}

#[allow(clippy::too_many_arguments)]
pub fn responses_metadata(
    installation_id: &str,
    session_id: &str,
    thread_id: &str,
    turn_id: Option<&str>,
    window_id: String,
    session_source: &SessionSource,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestForallResponsesRequestKind,
) -> ForallResponsesMetadata {
    let request_kind = match request_kind {
        TestForallResponsesRequestKind::Turn => Some(ForallResponsesRequestKind::Turn),
        TestForallResponsesRequestKind::Prewarm => Some(ForallResponsesRequestKind::Prewarm),
        TestForallResponsesRequestKind::WebsocketConnection => None,
    };
    ForallResponsesMetadata {
        turn_id: request_kind.and(turn_id.map(ToString::to_string)),
        request_kind,
        parent_thread_id,
        subagent_header: subagent_header_value(session_source),
        subagent_kind: request_kind.and_then(|_| subagent_metadata_kind(session_source)),
        ..ForallResponsesMetadata::new(
            installation_id.to_string(),
            session_id.to_string(),
            thread_id.to_string(),
            window_id,
        )
    }
}

pub fn all_model_presets() -> &'static Vec<ModelPreset> {
    &TEST_MODEL_PRESETS
}

pub fn builtin_collaboration_mode_presets() -> Vec<CollaborationModeMask> {
    collaboration_mode_presets::builtin_collaboration_mode_presets()
}
