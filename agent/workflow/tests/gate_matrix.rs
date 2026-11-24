//! VerifiedGate allow/block matrix and hook CLI parity.

mod support;

use support::{
    apply_patch_input, assert_gate_code, change_dir, complete_micro_change,
    complete_standard_change, evaluate_gate, exec_command_input, expect_allow, expect_block,
    hook_input, init_project, parse_json_output, propose_change, require_node,
    structured_write_input, write_markdown,
};

use forall_workflow::hooks::gate_error::GateErrorCode;
use forall_workflow::pre_tool_use_hook_output;
use forall_workflow::workflow::schema::load_workflow_schema;
use forall_workflow::workflow::state::compute_change_status;

#[test]
fn standard_change_not_apply_ready_until_design_and_verification() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let change = change_dir(root, "add-clamp");
    write_markdown(&change.join("proposal.md"), "# Why\n\nClamp.\n");
    write_markdown(
        &change.join("specs/math/spec.md"),
        "### Requirement: bounds\n\nSHALL clamp.\n",
    );
    write_markdown(&change.join("tasks.md"), "- [ ] Implement\n");

    let schema = load_workflow_schema(root).expect("schema");
    let status = compute_change_status(root, &schema, "add-clamp");
    assert!(
        !status.apply_ready,
        "standard change missing design/verification must not be apply-ready"
    );
    assert!(
        status.apply_blocked_by.contains(&"design".to_string())
            || status.artifacts.iter().any(|a| a.id == "design"
                && a.status != forall_workflow::workflow::types::ArtifactStatus::Done),
        "design should still be outstanding"
    );

    assert_gate_code(
        evaluate_gate(
            root,
            "apply_patch",
            apply_patch_input("src/clamp.ts", "+export {}\n"),
        ),
        GateErrorCode::ApplyNotReady,
    );
}

#[test]
fn apply_ready_standard_change_allows_app_code_patch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);
    complete_standard_change(root, "add-clamp");

    let schema = load_workflow_schema(root).expect("schema");
    assert!(
        compute_change_status(root, &schema, "add-clamp").apply_ready,
        "fixture should be apply-ready"
    );

    expect_allow(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input("src/clamp.ts", "+export function clamp() {}\n"),
    ));
}

#[test]
fn apply_ready_micro_change_allows_app_code_patch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "fix-copy", true);
    complete_micro_change(root, "fix-copy");

    let schema = load_workflow_schema(root).expect("schema");
    assert!(compute_change_status(root, &schema, "fix-copy").apply_ready);

    expect_allow(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input("src/ui.ts", "+export const label = 'Save';\n"),
    ));
}

#[test]
fn structured_write_tool_gated_like_apply_patch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    assert_gate_code(
        evaluate_gate(
            root,
            "write",
            structured_write_input("src/clamp.ts", "export {}\n"),
        ),
        GateErrorCode::ApplyNotReady,
    );

    complete_standard_change(root, "add-clamp");
    expect_allow(evaluate_gate(
        root,
        "write",
        structured_write_input("src/clamp.ts", "export function clamp() {}\n"),
    ));
}

#[test]
fn shell_write_patterns_block_before_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    for cmd in [
        "touch src/new.ts",
        "cp /tmp/x src/new.ts",
        "echo x > src/out.ts",
        "tee src/out.ts",
        "cat > src/hack.ts <<'EOF'\nline\nEOF",
        "git checkout -- src/clamp.ts",
        "git restore -- src/clamp.ts",
    ] {
        let result = evaluate_gate(root, "exec_command", exec_command_input(cmd));
        assert_eq!(
            expect_block(result).code,
            GateErrorCode::ShellWriteBlocked,
            "command should be blocked: {cmd}"
        );
    }
}

#[test]
fn read_only_shell_without_write_paths_is_allowed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    expect_allow(evaluate_gate(
        root,
        "exec_command",
        exec_command_input("git status && npm test"),
    ));
}

#[test]
fn multi_file_patch_blocked_if_any_path_is_app_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let patch = "*** Add File: notes.txt\n+ok\n*** Add File: src/clamp.ts\n+bad\n";
    let input = forall_workflow::hooks::paths::ToolInputCarrier {
        tool_input: Some(serde_json::json!({ "patch": patch })),
        tool_input_snake: None,
    };
    assert_gate_code(
        evaluate_gate(root, "apply_patch", input),
        GateErrorCode::ApplyNotReady,
    );
}

#[test]
fn hook_pre_tool_use_matches_gate_evaluator() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let input = hook_input(
        root,
        "apply_patch",
        serde_json::json!({
            "patch": "*** Add File: src/clamp.ts\n+export {}\n"
        }),
    );
    let hook_out = pre_tool_use_hook_output(&input);
    let hook_json = parse_json_output(&hook_out);

    let _gate_err = expect_block(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input("src/clamp.ts", "+export {}\n"),
    ));
    assert_eq!(
        hook_json.get("code").and_then(|v| v.as_str()),
        Some("APPLY_NOT_READY")
    );
    assert!(
        hook_json
            .get("blockedBy")
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("tasks")))
    );
    assert!(
        hook_json
            .get("remediation")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    );
}

#[test]
fn hook_pre_tool_use_allow_emits_empty_object() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);
    complete_standard_change(root, "add-clamp");

    let input = hook_input(
        root,
        "apply_patch",
        serde_json::json!({
            "patch": "*** Add File: src/clamp.ts\n+export function clamp() {}\n"
        }),
    );
    assert_eq!(pre_tool_use_hook_output(&input), "{}");
}

#[test]
fn scenario_failure_includes_full_stderr_when_node_present() {
    require_node();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    std::fs::create_dir_all(root.join(".forall/scenarios")).unwrap();
    std::fs::write(
        forall_workflow::paths::workflow_active_path(root),
        "add-clamp\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/clamp.ts"), "export {}\n").unwrap();

    let fingerprint = "GATE_MATRIX_SCENARIO_FINGERPRINT";
    let pad = "Q".repeat(900);
    std::fs::write(
        root.join(".forall/scenarios/clamp-bounds.scenario.mjs"),
        format!("throw new Error(\"{fingerprint}{pad}\");\n"),
    )
    .unwrap();
    std::fs::write(
        change_dir(root, "add-clamp").join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    verified: true
    property_tested: false
    code:
      file: src/clamp.ts
      symbols: [clamp]
"#,
    )
    .unwrap();

    let code = forall_workflow::commands::check::run_check(
        forall_workflow::commands::check::CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        },
    );
    assert_ne!(code, 0);

    let report = forall_workflow::verify::report::read_verify_report(root, Some("add-clamp"))
        .expect("report");
    let issue = report
        .issues
        .iter()
        .find(|i| i.phase == "scenario-tests")
        .expect("scenario-tests issue");
    assert!(issue.message.contains(fingerprint));
    assert!(issue.message.len() > 500);
}
