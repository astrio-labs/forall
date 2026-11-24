use super::new_status_output;
use crate::history_cell::HistoryCell;
use crate::legacy_core::config::Config;
use crate::legacy_core::config::ConfigBuilder;
use crate::status::StatusAccountDisplay;
use crate::test_support::PathBufExt;
use crate::test_support::test_path_buf;
use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;
use forall_models_manager::test_support::get_model_offline_for_tests;
use tempfile::TempDir;

async fn test_config(temp_home: &TempDir) -> Config {
    let cwd = tempfile::tempdir().expect("cwd");
    ConfigBuilder::default()
        .forall_home(temp_home.path().to_path_buf())
        .fallback_cwd(Some(cwd.path().to_path_buf()))
        .build()
        .await
        .expect("load config")
}

fn test_status_account_display() -> Option<StatusAccountDisplay> {
    Some(StatusAccountDisplay::ApiKey)
}

fn token_info_for(_model_slug: &str, usage: &TokenUsage) -> TokenUsageInfo {
    TokenUsageInfo {
        model_context_window: Some(200_000),
        total_token_usage: usage.clone(),
        last_token_usage: usage.clone(),
    }
}

#[tokio::test]
async fn status_output_renders_basic_fields() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("gpt-5.1-forall-max".to_string());
    config.model_provider_id = "openai".to_string();
    config.cwd = test_path_buf("/workspace/tests").abs();

    let usage = TokenUsage::default();
    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &usage);
    let composite = new_status_output(
        &config,
        test_status_account_display().as_ref(),
        Some(&token_info),
        &usage,
        &None,
        None,
        None,
        &model_slug,
        None,
        None,
    );
    let rendered = composite
        .display_lines(/*width*/ 80)
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("/status"));
    assert!(rendered.contains("gpt-5.1-forall-max"));
}
