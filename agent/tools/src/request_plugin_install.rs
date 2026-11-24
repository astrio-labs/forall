use std::collections::BTreeMap;

use forall_app_server_protocol::McpElicitationObjectType;
use forall_app_server_protocol::McpElicitationSchema;
use forall_app_server_protocol::McpServerElicitationRequest;
use forall_app_server_protocol::McpServerElicitationRequestParams;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

use crate::DiscoverableTool;
use crate::DiscoverableToolAction;
use crate::DiscoverableToolType;

pub const REQUEST_PLUGIN_INSTALL_APPROVAL_KIND_VALUE: &str = "tool_suggestion";
pub const REQUEST_PLUGIN_INSTALL_PERSIST_KEY: &str = "persist";
pub const REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE: &str = "always";

#[derive(Debug, Deserialize)]
pub struct RequestPluginInstallArgs {
    pub tool_type: DiscoverableToolType,
    pub action_type: DiscoverableToolAction,
    pub tool_id: String,
    pub suggest_reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RequestPluginInstallResult {
    pub completed: bool,
    pub user_confirmed: bool,
    pub tool_type: DiscoverableToolType,
    pub action_type: DiscoverableToolAction,
    pub tool_id: String,
    pub tool_name: String,
    pub suggest_reason: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RequestPluginInstallMeta<'a> {
    pub forall_approval_kind: &'static str,
    pub persist: &'static str,
    pub tool_type: DiscoverableToolType,
    pub suggest_type: DiscoverableToolAction,
    pub suggest_reason: &'a str,
    pub tool_id: &'a str,
    pub tool_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_plugin_id: Option<&'a str>,
}

pub fn build_request_plugin_install_elicitation_request(
    server_name: &str,
    thread_id: String,
    turn_id: String,
    suggest_reason: &str,
    tool: &DiscoverableTool,
) -> McpServerElicitationRequestParams {
    let message = suggest_reason.to_string();

    McpServerElicitationRequestParams {
        thread_id,
        turn_id: Some(turn_id),
        server_name: server_name.to_string(),
        request: McpServerElicitationRequest::Form {
            meta: Some(json!(build_request_plugin_install_meta(
                suggest_reason,
                tool,
            ))),
            message,
            requested_schema: McpElicitationSchema {
                schema_uri: None,
                type_: McpElicitationObjectType::Object,
                properties: BTreeMap::new(),
                required: None,
            },
        },
    }
}

fn build_request_plugin_install_meta<'a>(
    suggest_reason: &'a str,
    tool: &'a DiscoverableTool,
) -> RequestPluginInstallMeta<'a> {
    let (tool_type, remote_plugin_id) = match tool {
        DiscoverableTool::Plugin(plugin) => (
            DiscoverableToolType::Plugin,
            plugin.remote_plugin_id.as_deref(),
        ),
    };
    RequestPluginInstallMeta {
        forall_approval_kind: REQUEST_PLUGIN_INSTALL_APPROVAL_KIND_VALUE,
        persist: REQUEST_PLUGIN_INSTALL_PERSIST_ALWAYS_VALUE,
        tool_type,
        suggest_type: DiscoverableToolAction::Install,
        suggest_reason,
        tool_id: tool.id(),
        tool_name: tool.name(),
        install_url: tool.install_url(),
        remote_plugin_id,
    }
}

#[cfg(test)]
#[path = "request_plugin_install_tests.rs"]
mod tests;
