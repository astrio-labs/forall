use std::path::Path;

use forall_workflow::hooks::gate_error::serialize_pre_tool_use_block;
use forall_workflow::hooks::paths::ToolInputCarrier;
use forall_workflow::hooks::pre_tool_use_gate::{
    PreToolUseGateInput, PreToolUseGateResult, evaluate_pre_tool_use_gate,
};
use forall_workflow::hooks::verified_downgrade_gate::check_verified_downgrade_block;
use forall_workflow::paths::find_project_root;
use forall_workflow::workflow::schema::load_workflow_schema;
use serde_json::Value;

/// Native VerifiedGate (M0.2): blocks app-code mutations before optional hooks run.
pub fn run_verified_gate(
    workspace_root: &Path,
    tool_name: &str,
    tool_input: &Value,
) -> Option<String> {
    let project_root = find_project_root(workspace_root);
    let carrier = ToolInputCarrier {
        tool_input: Some(tool_input.clone()),
        tool_input_snake: None,
    };
    if let Some(project_root) = project_root.as_deref()
        && let Some(block) = check_verified_downgrade_block(project_root, tool_name, &carrier)
    {
        return Some(block);
    }
    match evaluate_pre_tool_use_gate(
        &PreToolUseGateInput {
            workspace_root,
            project_root: project_root.as_deref(),
            tool_name,
            tool_input: carrier,
        },
        load_workflow_schema,
    ) {
        PreToolUseGateResult::Allow => None,
        PreToolUseGateResult::Block(err) => Some(serialize_pre_tool_use_block(
            err.code,
            &err.message,
            &err.remediation,
            Some(err.paths),
            err.active_change,
            Some(err.blocked_by),
            err.next_artifact,
        )),
    }
}
