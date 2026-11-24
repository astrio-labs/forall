use serde_json::Value;

use crate::hooks::gate_error::serialize_pre_tool_use_block;
use crate::hooks::hook_input::{project_root, workspace_root};
use crate::hooks::paths::tool_input_from_json;
use crate::hooks::pre_tool_use_gate::{
    PreToolUseGateInput, PreToolUseGateResult, evaluate_pre_tool_use_gate,
};
use crate::hooks::verified_downgrade_gate::check_verified_downgrade_block;
use crate::workflow::schema::load_workflow_schema;

/// Hook stdout for `forall hook pre-tool-use` — same logic as the CLI entrypoint.
pub fn pre_tool_use_hook_output(input: &Value) -> String {
    let workspace_root = workspace_root(input);
    let project_root = project_root(input);
    let tool_name = input
        .get("toolName")
        .or_else(|| input.get("tool_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let carrier = tool_input_from_json(input.clone());

    if let Some(project_root) = project_root.as_deref()
        && let Some(block) = check_verified_downgrade_block(project_root, tool_name, &carrier)
    {
        return block;
    }

    let result = evaluate_pre_tool_use_gate(
        &PreToolUseGateInput {
            workspace_root: &workspace_root,
            project_root: project_root.as_deref(),
            tool_name,
            tool_input: carrier,
        },
        load_workflow_schema,
    );

    match result {
        PreToolUseGateResult::Allow => "{}".to_string(),
        PreToolUseGateResult::Block(err) => serialize_pre_tool_use_block(
            err.code,
            &err.message,
            &err.remediation,
            Some(err.paths),
            err.active_change,
            Some(err.blocked_by),
            err.next_artifact,
        ),
    }
}
