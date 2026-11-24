//! Shared fixtures for integration tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use forall_workflow::active_change::write_active_change_file;
use forall_workflow::commands::workflow::{run_init, run_propose};
use forall_workflow::hooks::gate_error::GateErrorCode;
use forall_workflow::hooks::paths::ToolInputCarrier;
use forall_workflow::hooks::pre_tool_use_gate::{
    PreToolUseGateInput, PreToolUseGateResult, evaluate_pre_tool_use_gate,
};
use forall_workflow::paths::{
    find_project_root, scenarios_dir as forall_scenarios_dir, verify_reports_dir,
};
use forall_workflow::workflow::schema::load_workflow_schema;

pub fn write_active_change(root: &Path, name: &str) {
    write_active_change_file(root, name).unwrap();
}

pub fn verify_report_path(root: &Path, change: &str) -> PathBuf {
    verify_reports_dir(root).join(format!("{change}-verify.json"))
}

pub fn write_markdown(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

pub fn init_project(root: &Path) {
    assert_eq!(run_init(root, false, false, false), 0, "run_init failed");
}

pub fn propose_change(root: &Path, name: &str, micro: bool) {
    assert_eq!(run_propose(root, name, micro), 0, "run_propose failed");
}

pub fn change_dir(root: &Path, name: &str) -> PathBuf {
    forall_workflow::workflow::paths::change_dir(root, name)
}

pub fn scenarios_dir(root: &Path) -> PathBuf {
    forall_scenarios_dir(root)
}

/// Standard workflow: all artifacts through tasks.
pub fn complete_standard_change(root: &Path, name: &str) {
    let dir = change_dir(root, name);
    write_markdown(&dir.join("proposal.md"), "# Why\n\nNeed clamp bounds.\n");
    write_markdown(
        &dir.join("specs/math/spec.md"),
        "### Requirement: clamp-bounds\n\nThe clamp SHALL stay within bounds.\n\n#### Scenario: in range\n\nWHEN x is between lo and hi THEN result is in range.\n",
    );
    write_markdown(
        &dir.join("design.md"),
        "## Context\n\nAdd clamp in src/clamp.ts.\n",
    );
    write_markdown(
        &dir.join("verification.md"),
        "## Properties\n\n- clamp-bounds: property test\n",
    );
    write_markdown(
        &dir.join("tasks.md"),
        "- [ ] Implement clamp\n- [ ] Run check\n",
    );
}

/// Micro workflow: proposal + specs + tasks only.
pub fn complete_micro_change(root: &Path, name: &str) {
    let dir = change_dir(root, name);
    write_markdown(&dir.join("proposal.md"), "# Why\n\nFix copy.\n");
    write_markdown(
        &dir.join("specs/ui/spec.md"),
        "### Requirement: label\n\nThe label SHALL read Save.\n",
    );
    write_markdown(&dir.join("tasks.md"), "- [ ] Update label\n");
}

pub fn apply_patch_input(relative_path: &str, body: &str) -> ToolInputCarrier {
    ToolInputCarrier {
        tool_input: Some(serde_json::json!({
            "patch": format!("*** Add File: {relative_path}\n{body}\n")
        })),
        tool_input_snake: None,
    }
}

pub fn exec_command_input(command: &str) -> ToolInputCarrier {
    ToolInputCarrier {
        tool_input: Some(serde_json::json!({ "cmd": command })),
        tool_input_snake: None,
    }
}

pub fn structured_write_input(path: &str, content: &str) -> ToolInputCarrier {
    ToolInputCarrier {
        tool_input: Some(serde_json::json!({
            "path": path,
            "content": content
        })),
        tool_input_snake: None,
    }
}

pub fn evaluate_gate(
    root: &Path,
    tool_name: &str,
    tool_input: ToolInputCarrier,
) -> PreToolUseGateResult {
    let project_root = find_project_root(root);
    evaluate_pre_tool_use_gate(
        &PreToolUseGateInput {
            workspace_root: root,
            project_root: project_root.as_deref(),
            tool_name,
            tool_input,
        },
        load_workflow_schema,
    )
}

pub fn hook_input(
    root: &Path,
    tool_name: &str,
    tool_input: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "cwd": root.display().to_string(),
        "toolName": tool_name,
        "toolInput": tool_input,
    })
}

pub fn expect_block(
    result: PreToolUseGateResult,
) -> forall_workflow::hooks::pre_tool_use_gate::GateError {
    match result {
        PreToolUseGateResult::Block(err) => err,
        PreToolUseGateResult::Allow => panic!("expected gate block, got allow"),
    }
}

pub fn expect_allow(result: PreToolUseGateResult) {
    assert!(
        matches!(result, PreToolUseGateResult::Allow),
        "expected gate allow, got block"
    );
}

pub fn assert_gate_code(result: PreToolUseGateResult, expected: GateErrorCode) {
    assert_eq!(expect_block(result).code, expected);
}

pub fn npm_available() -> bool {
    node_available()
        && Command::new("npm")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

pub fn write_clamp_project(root: &Path, change: &str, broken: bool) {
    init_project(root);
    fs::create_dir_all(change_dir(root, change)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(scenarios_dir(root)).unwrap();

    let clamp_src = if broken {
        "export function clamp(x: number, lo: number, hi: number): number {\n  return x;\n}\n"
    } else {
        "export function clamp(x: number, lo: number, hi: number): number {\n  if (x < lo) return lo;\n  if (x > hi) return hi;\n  return x;\n}\n"
    };
    fs::write(root.join("src/clamp.ts"), clamp_src).unwrap();
    fs::write(
        scenarios_dir(root).join("clamp-bounds.property.ts"),
        include_str!("../../schemas/forall/templates/clamp-bounds.property.ts"),
    )
    .unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"private":true,"type":"module","dependencies":{"fast-check":"^3.23.2"}}"#,
    )
    .unwrap();
    let npm = Command::new("npm")
        .args(["install", "--silent"])
        .current_dir(root)
        .status()
        .expect("npm install");
    assert!(npm.success(), "npm install failed for acceptance fixture");
    write_active_change(root, change);
    fs::write(
        change_dir(root, change).join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    property_tested: true
    property:
      file: .forall/scenarios/clamp-bounds.property.ts
      symbol: clamp
    code:
      file: src/clamp.ts
      symbols: [clamp]
"#,
    )
    .unwrap();
}

pub fn write_spec_tracked_project(root: &Path, change: &str) {
    init_project(root);
    fs::create_dir_all(change_dir(root, change)).unwrap();
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
    verified: false
    property_tested: false
    code:
      file: src/clamp.ts
      symbols: [clamp]
"#,
    )
    .unwrap();
}

pub fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Fail loudly when a runtime dependency is missing (no silent skip).
pub fn require_node() {
    assert!(
        node_available(),
        "integration test requires node.js on PATH (install node or use unit tests only)"
    );
}

pub fn require_python() {
    assert!(
        python_available(),
        "integration test requires python3 on PATH"
    );
}

pub fn parse_json_output(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw.trim()).expect("hook output must be valid JSON")
}

/// Run `forall hook pre-tool-use` as a subprocess with JSON on stdin (same as installed hooks).
pub fn run_hook_pre_tool_use_subprocess(
    root: &Path,
    hook_json: &serde_json::Value,
) -> std::process::Output {
    use std::io::Write;
    use std::process::{Command, Stdio};

    use forall_utils_cargo_bin::cargo_bin;

    let bin = cargo_bin("forall-workflow").expect("forall-workflow test binary");
    let mut child = Command::new(&bin)
        .args(["hook", "pre-tool-use"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to spawn {}: {err}", bin.display()));

    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(hook_json.to_string().as_bytes())
        .expect("write hook stdin");

    child.wait_with_output().expect("wait for hook subprocess")
}
