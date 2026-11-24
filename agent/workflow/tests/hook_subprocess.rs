//! Subprocess coverage for `forall hook pre-tool-use` (stdin JSON → stdout gate payload).

mod support;

use forall_workflow::pre_tool_use_hook_output;
use support::{
    complete_standard_change, hook_input, init_project, parse_json_output, propose_change,
    run_hook_pre_tool_use_subprocess,
};

#[test]
fn subprocess_block_matches_library_on_apply_not_ready() {
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
    let expected = pre_tool_use_hook_output(&input);
    assert_ne!(expected, "{}");

    let output = run_hook_pre_tool_use_subprocess(root, &input);
    assert!(
        output.status.success(),
        "hook subprocess should exit 0 even when blocking: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert_eq!(stdout.trim(), expected.trim());

    let json = parse_json_output(&stdout);
    assert_eq!(json.get("decision").and_then(|v| v.as_str()), Some("block"));
    assert_eq!(
        json.get("code").and_then(|v| v.as_str()),
        Some("APPLY_NOT_READY")
    );
}

#[test]
fn subprocess_allow_emits_empty_object() {
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

    let output = run_hook_pre_tool_use_subprocess(root, &input);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("utf8 stdout")
            .trim(),
        "{}"
    );
}

#[test]
fn subprocess_shell_write_block_matches_library() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let input = hook_input(
        root,
        "exec_command",
        serde_json::json!({ "cmd": "echo hacked > src/out.ts" }),
    );
    let expected = pre_tool_use_hook_output(&input);

    let output = run_hook_pre_tool_use_subprocess(root, &input);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("utf8 stdout")
            .trim(),
        expected.trim()
    );
    assert_eq!(
        parse_json_output(&expected)
            .get("code")
            .and_then(|v| v.as_str()),
        Some("SHELL_WRITE_BLOCKED")
    );
}

#[test]
fn subprocess_empty_stdin_allows_read_only_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    init_project(root);
    propose_change(root, "add-clamp", false);

    let output = run_hook_pre_tool_use_subprocess(root, &serde_json::json!({}));
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("utf8 stdout")
            .trim(),
        "{}"
    );
}
