#![allow(clippy::unwrap_used)]

use core::time::Duration;
use core_test_support::load_default_config_for_test;
use core_test_support::wait_for_event;
use forall_config::CONFIG_TOML_FILE;
use forall_core::NewThread;
use forall_features::Feature;
use forall_login::ForallAuth;
use forall_protocol::protocol::EventMsg;
use forall_protocol::protocol::InitialHistory;
use forall_protocol::protocol::WarningEvent;
use forall_utils_absolute_path::AbsolutePathBuf;
use tempfile::TempDir;
use tokio::time::timeout;
use toml::toml;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emits_warning_when_unstable_features_enabled_via_config() {
    let home = TempDir::new().expect("tempdir");
    let mut config = load_default_config_for_test(&home).await;
    config
        .features
        .enable(Feature::ApplyPatchStreamingEvents)
        .expect("test config should allow feature update");
    let user_config_path =
        AbsolutePathBuf::from_absolute_path(config.forall_home.join(CONFIG_TOML_FILE))
            .expect("absolute user config path");
    config.config_layer_stack = config.config_layer_stack.with_user_config(
        &user_config_path,
        toml! { features = { apply_patch_streaming_events = true } }.into(),
    );

    let thread_manager = forall_core::test_support::thread_manager_with_models_provider(
        ForallAuth::from_api_key("test"),
        config.model_provider.clone(),
    );
    let auth_manager =
        forall_core::test_support::auth_manager_from_auth(ForallAuth::from_api_key("test"));

    let NewThread {
        thread: conversation,
        ..
    } = thread_manager
        .resume_thread_with_history(
            config.clone(),
            InitialHistory::New,
            auth_manager,
            /*parent_trace*/ None,
            /*supports_openai_form_elicitation*/ false,
        )
        .await
        .expect("spawn conversation");

    let warning = wait_for_event(&conversation, |ev| matches!(ev, EventMsg::Warning(_))).await;
    let EventMsg::Warning(WarningEvent { message }) = warning else {
        panic!("expected warning event");
    };
    assert!(message.contains("apply_patch_streaming_events"));
    assert!(message.contains("Under-development features enabled"));
    assert!(message.contains("suppress_unstable_features_warning = true"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn suppresses_warning_when_configured() {
    let home = TempDir::new().expect("tempdir");
    let mut config = load_default_config_for_test(&home).await;
    config
        .features
        .enable(Feature::ApplyPatchStreamingEvents)
        .expect("test config should allow feature update");
    config.suppress_unstable_features_warning = true;
    let user_config_path =
        AbsolutePathBuf::from_absolute_path(config.forall_home.join(CONFIG_TOML_FILE))
            .expect("absolute user config path");
    config.config_layer_stack = config.config_layer_stack.with_user_config(
        &user_config_path,
        toml! { features = { apply_patch_streaming_events = true } }.into(),
    );

    let thread_manager = forall_core::test_support::thread_manager_with_models_provider(
        ForallAuth::from_api_key("test"),
        config.model_provider.clone(),
    );
    let auth_manager =
        forall_core::test_support::auth_manager_from_auth(ForallAuth::from_api_key("test"));

    let NewThread {
        thread: conversation,
        ..
    } = thread_manager
        .resume_thread_with_history(
            config.clone(),
            InitialHistory::New,
            auth_manager,
            /*parent_trace*/ None,
            /*supports_openai_form_elicitation*/ false,
        )
        .await
        .expect("spawn conversation");

    let warning = timeout(
        Duration::from_millis(150),
        wait_for_event(&conversation, |ev| matches!(ev, EventMsg::Warning(_))),
    )
    .await;
    assert!(warning.is_err());
}
