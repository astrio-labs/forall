//! Integration tests for native VerifiedGate in `forall-core` (tool dispatch path).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use forall_core::run_verified_gate;
use forall_workflow::commands::workflow::{run_init, run_propose};
use forall_workflow::hooks::gate_error::GateErrorCode;
use forall_workflow::pre_tool_use_hook_output;
use forall_workflow::workflow::paths::change_dir;

fn init_and_propose(root: &Path, change: &str) {
    assert_eq!(run_init(root, false, false, false), 0);
    assert_eq!(run_propose(root, change, false), 0);
}

fn write_partial_change(root: &Path, change: &str) {
    let dir = change_dir(root, change);
    fs::create_dir_all(dir.join("specs/math")).unwrap();
    fs::write(dir.join("proposal.md"), "# Why\n\nNeed clamp.\n").unwrap();
    fs::write(
        dir.join("specs/math/spec.md"),
        "### Requirement: bounds\n\nSHALL clamp.\n",
    )
    .unwrap();
    fs::write(dir.join("tasks.md"), "- [ ] Implement\n").unwrap();
}

fn complete_standard_change(root: &Path, change: &str) {
    let dir = change_dir(root, change);
    fs::create_dir_all(dir.join("specs/math")).unwrap();
    fs::write(dir.join("proposal.md"), "# Why\n\nNeed clamp bounds.\n").unwrap();
    fs::write(
        dir.join("specs/math/spec.md"),
        "### Requirement: clamp-bounds\n\nThe clamp SHALL stay within bounds.\n",
    )
    .unwrap();
    fs::write(
        dir.join("design.md"),
        "## Context\n\nAdd clamp in src/clamp.ts.\n",
    )
    .unwrap();
    fs::write(
        dir.join("verification.md"),
        "## Properties\n\n- clamp-bounds: property test\n",
    )
    .unwrap();
    fs::write(dir.join("tasks.md"), "- [ ] Implement clamp\n").unwrap();
}

fn apply_patch_input() -> serde_json::Value {
    serde_json::json!({
        "patch": "*** Add File: src/clamp.ts\n+export {}\n"
    })
}

fn hook_input(root: &Path, tool_input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "cwd": root.display().to_string(),
        "toolName": "apply_patch",
        "toolInput": tool_input,
    })
}

fn parse_block(payload: &str) -> serde_json::Value {
    let json: serde_json::Value =
        serde_json::from_str(payload.trim()).expect("gate payload must be JSON");
    assert_eq!(json.get("decision").and_then(|v| v.as_str()), Some("block"));
    json
}

#[test]
fn blocks_app_code_when_apply_not_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_and_propose(root, "add-clamp");
    write_partial_change(root, "add-clamp");

    let payload = run_verified_gate(root, "apply_patch", &apply_patch_input())
        .expect("expected VerifiedGate block");
    let json = parse_block(&payload);
    assert_eq!(
        json.get("code").and_then(|v| v.as_str()),
        Some(GateErrorCode::ApplyNotReady.as_str())
    );
    assert!(
        json.get("blockedBy")
            .and_then(|v| v.as_array())
            .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("tasks")))
    );
}

#[test]
fn allows_app_code_when_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_and_propose(root, "add-clamp");
    complete_standard_change(root, "add-clamp");

    assert!(run_verified_gate(root, "apply_patch", &apply_patch_input()).is_none());
}

#[test]
fn payload_matches_hook_and_library_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_and_propose(root, "add-clamp");
    write_partial_change(root, "add-clamp");

    let tool_input = apply_patch_input();
    let hook = hook_input(root, tool_input.clone());
    let from_hook = pre_tool_use_hook_output(&hook);
    let from_gate = run_verified_gate(root, "apply_patch", &tool_input).expect("block");

    assert_eq!(from_gate.trim(), from_hook.trim());
}

#[test]
fn blocks_shell_writes_before_apply_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_and_propose(root, "add-clamp");

    let tool_input = serde_json::json!({ "cmd": "touch src/new.ts" });
    let payload = run_verified_gate(root, "exec_command", &tool_input).expect("block");
    assert_eq!(
        parse_block(&payload).get("code").and_then(|v| v.as_str()),
        Some(GateErrorCode::ShellWriteBlocked.as_str())
    );
}

#[test]
fn read_only_tools_are_not_gated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_and_propose(root, "add-clamp");

    let tool_input = serde_json::json!({ "path": "README.md" });
    assert!(run_verified_gate(root, "read_file", &tool_input).is_none());
}
