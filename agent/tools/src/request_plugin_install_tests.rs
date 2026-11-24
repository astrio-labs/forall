use super::*;
use crate::DiscoverablePluginInfo;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn build_request_plugin_install_elicitation_request_injects_plugin_metadata() {
    let plugin = DiscoverableTool::Plugin(Box::new(DiscoverablePluginInfo {
        id: "sample@openai-curated-remote".to_string(),
        remote_plugin_id: Some("plugins~Plugin_sample".to_string()),
        name: "Sample Plugin".to_string(),
        description: Some("Includes skills and MCP servers.".to_string()),
        has_skills: true,
        mcp_server_names: vec!["sample-docs".to_string()],
    }));

    let request = build_request_plugin_install_elicitation_request(
        "forall-apps",
        "thread-1".to_string(),
        "turn-1".to_string(),
        "Use the sample plugin's skills and MCP server",
        &plugin,
    );

    assert_eq!(
        request,
        McpServerElicitationRequestParams {
            thread_id: "thread-1".to_string(),
            turn_id: Some("turn-1".to_string()),
            server_name: "forall-apps".to_string(),
            request: McpServerElicitationRequest::Form {
                meta: Some(json!(RequestPluginInstallMeta {
                    forall_approval_kind: REQUEST_PLUGIN_INSTALL_APPROVAL_KIND_VALUE,
                    persist: REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE,
                    tool_type: DiscoverableToolType::Plugin,
                    suggest_type: DiscoverableToolAction::Install,
                    suggest_reason: "Use the sample plugin's skills and MCP server",
                    tool_id: "sample@openai-curated-remote",
                    tool_name: "Sample Plugin",
                    install_url: None,
                    remote_plugin_id: Some("plugins~Plugin_sample"),
                })),
                message: "Use the sample plugin's skills and MCP server".to_string(),
                requested_schema: McpElicitationSchema {
                    schema_uri: None,
                    type_: McpElicitationObjectType::Object,
                    properties: BTreeMap::new(),
                    required: None,
                },
            },
        },
    );
}

#[test]
fn build_request_plugin_install_meta_uses_expected_shape() {
    let plugin = DiscoverableTool::Plugin(Box::new(DiscoverablePluginInfo {
        id: "sample@openai-curated-remote".to_string(),
        remote_plugin_id: Some("plugins~Plugin_sample".to_string()),
        name: "Sample Plugin".to_string(),
        description: None,
        has_skills: false,
        mcp_server_names: Vec::new(),
    }));
    let meta =
        build_request_plugin_install_meta("Use the sample plugin's skills and MCP server", &plugin);

    assert_eq!(
        meta,
        RequestPluginInstallMeta {
            forall_approval_kind: REQUEST_PLUGIN_INSTALL_APPROVAL_KIND_VALUE,
            persist: REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE,
            tool_type: DiscoverableToolType::Plugin,
            suggest_type: DiscoverableToolAction::Install,
            suggest_reason: "Use the sample plugin's skills and MCP server",
            tool_id: "sample@openai-curated-remote",
            tool_name: "Sample Plugin",
            install_url: None,
            remote_plugin_id: Some("plugins~Plugin_sample"),
        },
    );
}
