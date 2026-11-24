//! Verification adapter registry — honest multi-language check behavior.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::commands::workflow::run_init;

mod support;
use support::{change_dir, verify_report_path, write_active_change};

fn write_python_project(root: &Path, change: &str, verified: bool) {
    run_init(root, false, false, false);
    fs::create_dir_all(change_dir(root, change)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/demo.py"), "def demo():\n    return 1\n").unwrap();
    write_active_change(root, change);

    let verified_line = if verified {
        "    verified: true"
    } else {
        "    verified: false"
    };

    fs::write(
        change_dir(root, change).join("mapping.delta.yaml"),
        format!(
            r#"version: 1
requirements:
  - id: py-demo
    capability: demo
    requirement: demo behavior
{verified_line}
    code:
      file: src/demo.py
      symbols: [demo]
"#
        ),
    )
    .unwrap();
}

#[test]
fn verified_false_python_skips_dafny_proof_phase() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "py-spec-only";
    write_python_project(root, change, false);

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some(change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    assert_eq!(code, 0, "spec-tracked python should not fail adapter gate");

    let raw = fs::read_to_string(verify_report_path(root, change)).expect("verify report");
    assert!(
        !raw.contains("proof check failed"),
        "python verified:false must not run Dafny proofs"
    );
    assert!(
        !raw.contains("proof engine failed"),
        "python verified:false must not run LemmaScript extract for proofs"
    );
}

#[test]
fn verified_true_python_fails_with_adapter_message() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "py-verified-blocked";
    write_python_project(root, change, true);

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some(change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    assert_ne!(
        code, 0,
        "verified:true on .py should fail until a proof adapter exists"
    );

    let raw = fs::read_to_string(verify_report_path(root, change)).expect("verify report");
    assert!(
        raw.contains("proof: none") || raw.contains("no verification adapter"),
        "expected adapter failure message, got: {raw}"
    );
}
