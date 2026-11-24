use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn discoverable_tool_enums_use_expected_wire_names() {
    assert_eq!(
        json!({
            "tool_type": DiscoverableToolType::Plugin,
            "action_type": DiscoverableToolAction::Install,
        }),
        json!({
            "tool_type": "plugin",
            "action_type": "install",
        })
    );
}

#[test]
fn filter_request_plugin_install_discoverable_tools_for_forall_tui_omits_plugins() {
    let discoverable_tools = vec![DiscoverableTool::Plugin(Box::new(DiscoverablePluginInfo {
        id: "slack@openai-curated".to_string(),
        remote_plugin_id: None,
        name: "Slack".to_string(),
        description: Some("Search Slack messages".to_string()),
        has_skills: true,
        mcp_server_names: vec!["slack".to_string()],
    }))];

    assert_eq!(
        filter_request_plugin_install_discoverable_tools_for_client(
            discoverable_tools,
            Some("forall-tui"),
        ),
        Vec::<DiscoverableTool>::new()
    );
}
