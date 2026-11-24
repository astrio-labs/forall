use crate::active_change::resolve_active_change;
use crate::commands::workflow::{
    ensure_c_for_agent, ensure_java_for_agent, ensure_lsc_for_agent, ensure_rust_verification,
    ensure_verified_project, ensure_verus_for_agent,
};
use crate::hooks::gate_error::GateErrorCode;
use crate::hooks::paths::{
    ToolInputCarrier, extract_edit_paths_from_tool, is_application_code_path,
    is_pre_tool_use_gated_tool,
};
use crate::paths::find_project_root;

use crate::workflow::state::{compute_change_status, next_ready_artifact};
use crate::workflow::types::WorkflowSchema;
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static SHELL_WRITE_TOOL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)exec_command|shell|bash|zsh")
        .unwrap_or_else(|err| panic!("invalid shell write tool regex: {err}"))
});

fn map_block_code(tool_name: &str, code: GateErrorCode) -> GateErrorCode {
    if SHELL_WRITE_TOOL.is_match(tool_name)
        && matches!(
            code,
            GateErrorCode::NoProjectRoot
                | GateErrorCode::NoActiveChange
                | GateErrorCode::ApplyNotReady
        )
    {
        GateErrorCode::ShellWriteBlocked
    } else {
        code
    }
}

#[allow(clippy::too_many_arguments)]
fn block_error(
    tool_name: &str,
    code: GateErrorCode,
    message: String,
    remediation: String,
    paths: Vec<String>,
    active_change: Option<String>,
    blocked_by: Vec<String>,
    next_artifact: Option<String>,
) -> PreToolUseGateResult {
    PreToolUseGateResult::Block(GateError {
        code: map_block_code(tool_name, code),
        message,
        remediation,
        paths,
        active_change,
        blocked_by,
        next_artifact,
    })
}

pub struct PreToolUseGateInput<'a> {
    pub workspace_root: &'a Path,
    pub project_root: Option<&'a Path>,
    pub tool_name: &'a str,
    pub tool_input: ToolInputCarrier,
}

#[derive(Debug, Clone)]
pub struct GateError {
    pub code: GateErrorCode,
    pub message: String,
    pub remediation: String,
    pub paths: Vec<String>,
    pub active_change: Option<String>,
    pub blocked_by: Vec<String>,
    pub next_artifact: Option<String>,
}

pub enum PreToolUseGateResult {
    Allow,
    Block(GateError),
}

pub fn evaluate_pre_tool_use_gate(
    input: &PreToolUseGateInput<'_>,
    load_schema: impl FnOnce(&Path) -> anyhow::Result<WorkflowSchema>,
) -> PreToolUseGateResult {
    if !is_pre_tool_use_gated_tool(input.tool_name) {
        return PreToolUseGateResult::Allow;
    }

    let path_root = input.project_root.unwrap_or(input.workspace_root);
    let paths = extract_edit_paths_from_tool(path_root, input.tool_name, &input.tool_input);
    let code_paths: Vec<_> = paths
        .iter()
        .filter(|p| is_application_code_path(p))
        .cloned()
        .collect();
    if code_paths.is_empty() {
        return PreToolUseGateResult::Allow;
    }

    let project_root = input
        .project_root
        .map(std::path::Path::to_path_buf)
        .or_else(|| find_project_root(input.workspace_root));

    let Some(project_root) = project_root else {
        // M0.4: init/bootstrap on first app-code mutation, then re-evaluate (never silent allow).
        let ensure = ensure_verified_project(input.workspace_root);
        if !ensure.ok {
            return block_error(
                input.tool_name,
                GateErrorCode::EnsureFailed,
                format!(
                    "Forall: could not initialize the project for application code edits ({}).",
                    code_paths.join(", ")
                ),
                "Run `forall init` manually, fix any errors, then `forall propose <name>` before editing application code."
                    .to_string(),
                code_paths,
                None,
                Vec::new(),
                None,
            );
        }
        let Some(root) = find_project_root(input.workspace_root) else {
            return block_error(
                input.tool_name,
                GateErrorCode::NoProjectRoot,
                format!(
                    "Forall: editing application code ({}) requires a Forall project (run `forall init` or `forall ensure`).",
                    code_paths.join(", ")
                ),
                "Run `forall init` (or `forall ensure`) in this workspace, then `forall propose <name>` before editing src/ or other app paths."
                    .to_string(),
                code_paths,
                None,
                Vec::new(),
                None,
            );
        };
        return evaluate_pre_tool_use_gate(
            &PreToolUseGateInput {
                workspace_root: input.workspace_root,
                project_root: Some(&root),
                tool_name: input.tool_name,
                tool_input: input.tool_input.clone(),
            },
            load_schema,
        );
    };

    let Some(change) = resolve_active_change(&project_root) else {
        return block_error(
            input.tool_name,
            GateErrorCode::NoActiveChange,
            format!(
                "Forall: editing {} requires an active change.",
                code_paths.join(", ")
            ),
            "Run `forall propose <name>` (or `specs.propose`) to start a tracked change before editing application code."
                .to_string(),
            code_paths,
            None,
            Vec::new(),
            None,
        );
    };

    let Ok(schema) = load_schema(&project_root) else {
        return PreToolUseGateResult::Allow;
    };
    let status = compute_change_status(&project_root, &schema, &change);
    if !status.apply_ready {
        let next = next_ready_artifact(&status);
        return block_error(
            input.tool_name,
            GateErrorCode::ApplyNotReady,
            format!(
                "Forall: apply not ready for change '{change}' (blocked by: {}).",
                status.apply_blocked_by.join(", ")
            ),
            if let Some(next) = &next {
                format!(
                    "Call `forall instructions {next} --change {change}` and complete that artifact before editing application code. Use apply_patch on .forall/changes/ for workflow artifacts."
                )
            } else {
                "Complete remaining workflow artifacts before editing application code.".to_string()
            },
            code_paths,
            Some(change),
            status.apply_blocked_by,
            next,
        );
    }

    if code_paths.iter().any(|p| p.ends_with(".rs")) {
        let _ = ensure_rust_verification(&project_root, false);
        let _ = ensure_verus_for_agent();
    }

    if code_paths.iter().any(|p| p.ends_with(".java")) {
        let _ = ensure_java_for_agent(&project_root);
    }

    if code_paths.iter().any(|p| p.ends_with(".c")) {
        let _ = ensure_c_for_agent(&project_root);
    }

    if code_paths
        .iter()
        .any(|p| p.ends_with(".ts") || p.ends_with(".tsx"))
    {
        let _ = ensure_lsc_for_agent();
    }

    PreToolUseGateResult::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::workflow::types::{ApplyConfig, Artifact, WorkflowSchema};

    fn schema() -> WorkflowSchema {
        WorkflowSchema {
            name: "forall".to_string(),
            version: 1,
            description: None,
            artifacts: vec![
                Artifact {
                    id: "proposal".to_string(),
                    generates: "proposal.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec![],
                },
                Artifact {
                    id: "tasks".to_string(),
                    generates: "tasks.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec!["proposal".to_string()],
                },
            ],
            apply: Some(ApplyConfig {
                requires: vec!["tasks".to_string()],
                tracks: "tasks.md".to_string(),
                instruction: None,
            }),
        }
    }

    #[test]
    fn allows_read_only_tools() {
        let result = evaluate_pre_tool_use_gate(
            &PreToolUseGateInput {
                workspace_root: Path::new("/tmp/ws"),
                project_root: None,
                tool_name: "read_file",
                tool_input: ToolInputCarrier::default(),
            },
            |_| Ok(schema()),
        );
        assert!(matches!(result, PreToolUseGateResult::Allow));
    }

    #[test]
    fn blocks_app_code_without_project() {
        crate::test_env::with_forall_change(None, || {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path();
            let result = evaluate_pre_tool_use_gate(
                &PreToolUseGateInput {
                    workspace_root: root,
                    project_root: None,
                    tool_name: "apply_patch",
                    tool_input: ToolInputCarrier {
                        tool_input: Some(serde_json::json!({
                            "patch": "*** Add File: src/foo.ts\n+export {}\n"
                        })),
                        tool_input_snake: None,
                    },
                },
                |_| Ok(schema()),
            );
            match result {
                PreToolUseGateResult::Block(err) => {
                    assert_eq!(err.code, GateErrorCode::NoActiveChange);
                    assert!(err.paths.contains(&"src/foo.ts".to_string()));
                    assert!(err.remediation.contains("propose"));
                    assert!(
                        crate::paths::verify_mapping_path(root).is_file(),
                        "gate should run forall init before blocking"
                    );
                }
                PreToolUseGateResult::Allow => panic!("expected block"),
            }
        });
    }

    #[test]
    fn allows_planning_paths_without_project() {
        let result = evaluate_pre_tool_use_gate(
            &PreToolUseGateInput {
                workspace_root: Path::new("/tmp/ws"),
                project_root: None,
                tool_name: "apply_patch",
                tool_input: ToolInputCarrier {
                    tool_input: Some(serde_json::json!({
                        "patch": "*** Add File: notes.txt\n+hello\n"
                    })),
                    tool_input_snake: None,
                },
            },
            |_| Ok(schema()),
        );
        assert!(matches!(result, PreToolUseGateResult::Allow));
    }

    #[test]
    fn blocks_app_code_when_apply_not_ready() {
        use crate::commands::workflow::{run_init, run_propose};
        use crate::paths::find_project_root;
        use crate::workflow::schema::load_workflow_schema;

        crate::test_env::with_forall_change(None, || {
            let dir = tempfile::tempdir().expect("tempdir");
            assert_eq!(run_init(dir.path(), false, false, false), 0);
            assert_eq!(run_propose(dir.path(), "add-clamp", false), 0);
            let root = find_project_root(dir.path()).expect("project root");

            let result = evaluate_pre_tool_use_gate(
                &PreToolUseGateInput {
                    workspace_root: dir.path(),
                    project_root: Some(&root),
                    tool_name: "apply_patch",
                    tool_input: ToolInputCarrier {
                        tool_input: Some(serde_json::json!({
                            "command": "*** Add File: src/clamp.ts\n+export {}\n"
                        })),
                        tool_input_snake: None,
                    },
                },
                load_workflow_schema,
            );
            match result {
                PreToolUseGateResult::Block(err) => {
                    assert_eq!(err.code, GateErrorCode::ApplyNotReady);
                    assert!(err.blocked_by.contains(&"tasks".to_string()));
                }
                PreToolUseGateResult::Allow => panic!("expected APPLY_NOT_READY block"),
            }
        });
    }

    #[test]
    fn blocks_native_author_property_when_apply_not_ready() {
        use crate::commands::workflow::{run_init, run_propose};
        use crate::paths::find_project_root;
        use crate::workflow::schema::load_workflow_schema;

        crate::test_env::with_forall_change(None, || {
            let dir = tempfile::tempdir().expect("tempdir");
            assert_eq!(run_init(dir.path(), false, false, false), 0);
            assert_eq!(run_propose(dir.path(), "add-clamp", false), 0);
            let root = find_project_root(dir.path()).expect("project root");
            let result = evaluate_pre_tool_use_gate(
                &PreToolUseGateInput {
                    workspace_root: dir.path(),
                    project_root: Some(&root),
                    tool_name: "author.scaffold_property",
                    tool_input: ToolInputCarrier {
                        tool_input: Some(serde_json::json!({
                            "mode": "apply",
                            "paths": [
                                ".forall/verify/mapping.yaml",
                                ".forall/scenarios/clamp-bounds.property.ts"
                            ]
                        })),
                        tool_input_snake: None,
                    },
                },
                load_workflow_schema,
            );
            match result {
                PreToolUseGateResult::Block(err) => {
                    assert_eq!(err.code, GateErrorCode::ApplyNotReady);
                    assert!(
                        err.paths
                            .contains(&".forall/scenarios/clamp-bounds.property.ts".to_string())
                    );
                }
                PreToolUseGateResult::Allow => panic!("expected APPLY_NOT_READY block"),
            }
        });
    }

    #[test]
    fn allows_planning_path_with_active_change() {
        use crate::commands::workflow::{run_init, run_propose};
        use crate::paths::find_project_root;
        use crate::workflow::schema::load_workflow_schema;

        crate::test_env::with_forall_change(None, || {
            let dir = tempfile::tempdir().expect("tempdir");
            assert_eq!(run_init(dir.path(), false, false, false), 0);
            assert_eq!(run_propose(dir.path(), "add-clamp", false), 0);
            let root = find_project_root(dir.path()).expect("project root");

            let result = evaluate_pre_tool_use_gate(
                &PreToolUseGateInput {
                    workspace_root: dir.path(),
                    project_root: Some(&root),
                    tool_name: "apply_patch",
                    tool_input: ToolInputCarrier {
                        tool_input: Some(serde_json::json!({
                            "command": "*** Add File: .forall/workflow/changes/add-clamp/specs/math/spec.md\n+# spec\n"
                        })),
                        tool_input_snake: None,
                    },
                },
                load_workflow_schema,
            );
            assert!(matches!(result, PreToolUseGateResult::Allow));
        });
    }
}
