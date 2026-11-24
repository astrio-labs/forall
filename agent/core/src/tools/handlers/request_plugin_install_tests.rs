use super::*;
use crate::plugins::test_support::load_plugins_config;
use crate::plugins::test_support::write_curated_plugin_sha;
use crate::plugins::test_support::write_openai_curated_marketplace;
use crate::plugins::test_support::write_plugins_feature_config;
use core_test_support::PathExt;
use forall_config::CONFIG_TOML_FILE;
use forall_config::config_toml::ConfigToml;
use forall_config::types::ToolSuggestConfig;
use forall_config::types::ToolSuggestDisabledTool;
use forall_config::types::ToolSuggestDiscoverable;
use forall_config::types::ToolSuggestDiscoverableType;
use forall_core_plugins::PluginInstallRequest;
use forall_core_plugins::PluginsManager;
use forall_core_plugins::startup_sync::curated_plugins_repo_path;
use forall_rmcp_client::ElicitationResponse;
use forall_tools::DiscoverablePluginInfo;
use forall_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use rmcp::model::ElicitationAction;
use serde_json::json;
use tempfile::tempdir;

#[tokio::test]
async fn verified_plugin_install_completed_requires_installed_plugin() {
    let forall_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(forall_home.path());
    write_openai_curated_marketplace(&curated_root, &["sample"]);
    write_curated_plugin_sha(forall_home.path());
    write_plugins_feature_config(forall_home.path());

    let config = load_plugins_config(forall_home.path()).await;
    let plugins_manager = PluginsManager::new(forall_home.path().to_path_buf());

    assert!(!verified_plugin_install_completed(
        "sample@openai-curated",
        &config,
        &plugins_manager,
    ));

    plugins_manager
        .install_plugin(PluginInstallRequest {
            plugin_name: "sample".to_string(),
            marketplace_path: AbsolutePathBuf::try_from(
                curated_root.join(".agents/plugins/marketplace.json"),
            )
            .expect("marketplace path"),
        })
        .await
        .expect("plugin should install");

    let refreshed_config = load_plugins_config(forall_home.path()).await;
    assert!(verified_plugin_install_completed(
        "sample@openai-curated",
        &refreshed_config,
        &plugins_manager,
    ));
}

#[test]
fn remote_plugin_install_suggestions_skip_core_installed_verification() {
    assert!(is_remote_plugin_install_suggestion(
        "snowflake@openai-curated-remote"
    ));
    assert!(!is_remote_plugin_install_suggestion(
        "snowflake@openai-curated"
    ));
    assert!(!is_remote_plugin_install_suggestion("Plugin_123"));
}

#[test]
fn recommended_plugin_install_args_accept_legacy_tool_id() {
    let current: RecommendedPluginInstallArgs = serde_json::from_value(json!({
        "plugin_id": "google-drive@openai-curated-remote",
        "suggest_reason": "Use Google Drive for this request"
    }))
    .expect("current arguments should deserialize");
    let legacy: RecommendedPluginInstallArgs = serde_json::from_value(json!({
        "tool_type": "plugin",
        "action_type": "install",
        "tool_id": "google-drive@openai-curated-remote",
        "suggest_reason": "Use Google Drive for this request"
    }))
    .expect("legacy arguments should deserialize");

    assert_eq!(current, legacy);
}

#[test]
fn request_plugin_install_response_persists_only_decline_always_mode() {
    assert!(request_plugin_install_response_requests_persistent_disable(
        &ElicitationResponse {
            action: ElicitationAction::Decline,
            content: None,
            meta: Some(json!({
                REQUEST_PLUGIN_INSTALL_PERSIST_KEY: REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE
            })),
        }
    ));
    assert!(
        !request_plugin_install_response_requests_persistent_disable(&ElicitationResponse {
            action: ElicitationAction::Accept,
            content: None,
            meta: Some(json!({
                REQUEST_PLUGIN_INSTALL_PERSIST_KEY: REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE
            })),
        })
    );
    assert!(
        !request_plugin_install_response_requests_persistent_disable(&ElicitationResponse {
            action: ElicitationAction::Decline,
            content: None,
            meta: Some(json!({ REQUEST_PLUGIN_INSTALL_PERSIST_KEY: "session" })),
        })
    );
    assert!(
        !request_plugin_install_response_requests_persistent_disable(&ElicitationResponse {
            action: ElicitationAction::Decline,
            content: None,
            meta: None,
        })
    );
}

#[tokio::test]
async fn persist_disabled_install_request_writes_plugin_config() {
    let forall_home = tempdir().expect("tempdir should succeed");
    let tool = plugin_tool("slack@openai-curated", "Slack");

    persist_disabled_install_request(&forall_home.path().abs(), &tool)
        .await
        .expect("persist plugin disable");

    let contents =
        std::fs::read_to_string(forall_home.path().join(CONFIG_TOML_FILE)).expect("read config");
    let parsed: ConfigToml = toml::from_str(&contents).expect("parse config");
    assert_eq!(
        parsed.tool_suggest,
        Some(ToolSuggestConfig {
            discoverables: Vec::new(),
            disabled_tools: vec![ToolSuggestDisabledTool::plugin("slack@openai-curated")],
        })
    );
}

#[tokio::test]
async fn persist_disabled_install_request_dedupes_existing_disabled_tools() {
    let forall_home = tempdir().expect("tempdir should succeed");
    let tool = plugin_tool("slack@openai-curated", "Slack");
    std::fs::write(
        forall_home.path().join(CONFIG_TOML_FILE),
        r#"
[tool_suggest]
discoverables = [
  { type = "plugin", id = "sample@openai-curated" }
]

[[tool_suggest.disabled_tools]]
type = "plugin"
id = " slack@openai-curated "

[[tool_suggest.disabled_tools]]
type = "plugin"
id = "slack@openai-curated"

[[tool_suggest.disabled_tools]]
type = "plugin"
id = "   "

[[tool_suggest.disabled_tools]]
type = "connector"
id = "connector_calendar"
"#,
    )
    .expect("write config");

    persist_disabled_install_request(&forall_home.path().abs(), &tool)
        .await
        .expect("persist plugin disable");

    let contents =
        std::fs::read_to_string(forall_home.path().join(CONFIG_TOML_FILE)).expect("read config");
    let parsed: ConfigToml = toml::from_str(&contents).expect("parse config");
    assert_eq!(
        parsed.tool_suggest,
        Some(ToolSuggestConfig {
            discoverables: vec![ToolSuggestDiscoverable {
                kind: ToolSuggestDiscoverableType::Plugin,
                id: "sample@openai-curated".to_string(),
            }],
            disabled_tools: vec![
                ToolSuggestDisabledTool::plugin("slack@openai-curated"),
                ToolSuggestDisabledTool::connector("connector_calendar"),
            ],
        })
    );
}

fn plugin_tool(id: &str, name: &str) -> DiscoverableTool {
    DiscoverableTool::Plugin(Box::new(DiscoverablePluginInfo {
        id: id.to_string(),
        remote_plugin_id: None,
        name: name.to_string(),
        description: None,
        has_skills: false,
        mcp_server_names: Vec::new(),
    }))
}
