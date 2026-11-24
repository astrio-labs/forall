//! VerifiedGate and check diagnostics acceptance tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]
mod support;

use std::fs;
use std::path::Path;

use support::{
    apply_patch_input, change_dir, evaluate_gate, exec_command_input, expect_allow, expect_block,
    hook_input, init_project, parse_json_output, propose_change, require_node, scenarios_dir,
    write_active_change,
};

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::hooks::gate_error::{GateErrorCode, serialize_pre_tool_use_block};
use forall_workflow::mapping::schema::{IssueSeverity, VerifyReport};
use forall_workflow::paths::find_project_root;
use forall_workflow::pre_tool_use_hook_output;
use forall_workflow::verify::report::read_verify_report;

const SCENARIO_DIAGNOSTIC_FINGERPRINT: &str = "SCENARIO_DIAGNOSTIC_FINGERPRINT_ZZZ_";

fn parse_gate_block(payload: &str) -> serde_json::Value {
    let json = parse_json_output(payload);
    assert_eq!(json.get("decision").and_then(|v| v.as_str()), Some("block"));
    assert!(
        json.get("remediation")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty())
    );
    json
}

fn scenario_test_issue(report: &VerifyReport) -> &forall_workflow::mapping::schema::VerifyIssue {
    report
        .issues
        .iter()
        .find(|i| i.phase == "scenario-tests" && i.severity == IssueSeverity::Critical)
        .expect("expected scenario-tests CRITICAL issue")
}

fn write_scenario_diagnostic_project(root: &Path, change: &str) {
    init_project(root);
    fs::create_dir_all(change_dir(root, change)).unwrap();
    fs::create_dir_all(scenarios_dir(root)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();

    fs::write(
        root.join("src/clamp.ts"),
        "export function clamp(x: number, lo: number, hi: number): number {\n  return x;\n}\n",
    )
    .unwrap();
    write_active_change(root, change);
    fs::write(
        change_dir(root, change).join("mapping.delta.yaml"),
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

    let pad = "Z".repeat(1200);
    fs::write(
        root.join(".forall/scenarios/clamp-bounds.scenario.mjs"),
        format!(
            "const fingerprint = \"{SCENARIO_DIAGNOSTIC_FINGERPRINT}\" + \"{pad}\";\nthrow new Error(fingerprint);\n"
        ),
    )
    .unwrap();
}

#[test]
fn init_project_on_app_code_patch_then_blocks_without_active_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    assert!(find_project_root(root).is_none());

    let target = root.join("src/foo.ts");
    let err = expect_block(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input("src/foo.ts", "+export {}\n"),
    ));
    assert_eq!(err.code, GateErrorCode::NoActiveChange);
    assert!(err.remediation.contains("propose"));

    let payload = serialize_pre_tool_use_block(
        err.code,
        &err.message,
        &err.remediation,
        Some(err.paths),
        err.active_change,
        Some(err.blocked_by),
        err.next_artifact,
    );
    assert_eq!(
        parse_gate_block(&payload)
            .get("code")
            .and_then(|v| v.as_str()),
        Some("NO_ACTIVE_CHANGE")
    );

    assert!(forall_workflow::paths::verify_mapping_path(root).is_file());
    assert!(!target.exists());
}

#[test]
fn hook_matches_gate_on_first_mutation_without_project() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    let input = hook_input(
        root,
        "apply_patch",
        serde_json::json!({
            "patch": "*** Add File: src/foo.ts\n+export {}\n"
        }),
    );
    let hook_json = parse_json_output(&pre_tool_use_hook_output(&input));
    assert_eq!(
        hook_json.get("code").and_then(|v| v.as_str()),
        Some("NO_ACTIVE_CHANGE")
    );
}

#[test]
fn blocks_shell_sed_write_until_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let err = expect_block(evaluate_gate(
        root,
        "exec_command",
        exec_command_input("sed -i '' src/clamp.ts"),
    ));
    assert_eq!(err.code, GateErrorCode::ShellWriteBlocked);
    assert!(err.blocked_by.contains(&"tasks".to_string()));
}

#[test]
fn blocks_app_code_patch_until_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let err = expect_block(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input("src/clamp.ts", "+export {}\n"),
    ));
    assert_eq!(err.code, GateErrorCode::ApplyNotReady);

    let payload = serialize_pre_tool_use_block(
        err.code,
        &err.message,
        &err.remediation,
        Some(err.paths),
        err.active_change,
        Some(err.blocked_by.clone()),
        err.next_artifact,
    );
    let json = parse_gate_block(&payload);
    assert!(
        json.get("blockedBy")
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("tasks")))
    );
}

#[test]
fn allows_change_artifact_patch_before_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let planning_rel = ".forall/workflow/changes/add-clamp/specs/math/spec.md";
    expect_allow(evaluate_gate(
        root,
        "apply_patch",
        apply_patch_input(planning_rel, "+# spec\n"),
    ));
}

#[test]
fn check_report_keeps_full_scenario_failure_output() {
    require_node();

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_scenario_diagnostic_project(root, "add-clamp");

    assert_ne!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some("add-clamp"),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        }),
        0
    );

    let report = read_verify_report(root, Some("add-clamp")).expect("report");
    let issue = scenario_test_issue(&report);
    assert!(issue.message.len() > 500);
    assert!(issue.message.contains(SCENARIO_DIAGNOSTIC_FINGERPRINT));
    assert!(issue.message.contains(&"Z".repeat(200)));
}
