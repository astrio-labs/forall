//! OpenJML adapter registry and contract extraction tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::commands::workflow::ensure_java_verification;
use forall_workflow::verify::adapters::{
    PROOF_OPENJML, ensure_java_adapter_persisted, load_verified_config,
};
use forall_workflow::verify::backends::openjml::extract_jml_contracts;

mod support;
use support::{change_dir, init_project, verify_report_path, write_active_change};

fn write_java_project(root: &std::path::Path, change: &str, verified: bool) {
    init_project(root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/Clamp.java"),
        r#"public class Clamp {
    //@ requires lo <= hi;
    //@ ensures lo <= \result && \result <= hi;
    public static int clamp(int x, int lo, int hi) {
        if (x < lo) return lo;
        if (x > hi) return hi;
        return x;
    }
}
"#,
    )
    .unwrap();
    fs::create_dir_all(change_dir(root, change)).unwrap();
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
  - id: clamp-bounds
    capability: math
    requirement: clamp within bounds
{verified_line}
    code:
      file: src/Clamp.java
      symbols: [clamp]
    contract: |
      requires lo <= hi
      ensures lo <= result <= hi
"#
        ),
    )
    .unwrap();
}

#[test]
fn default_config_registers_openjml_for_java() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let config = load_verified_config(dir.path());
    let (_, adapter) = config.adapter_for_file("src/Foo.java").unwrap();
    assert_eq!(adapter.proof, PROOF_OPENJML);
}

#[test]
fn openjml_extractor_finds_requires_and_ensures() {
    let source = r#"
//@ requires lo <= hi;
//@ ensures lo <= \result && \result <= hi;
public static int clamp(int x, int lo, int hi) {
    return x;
}
"#;
    let extract = extract_jml_contracts(source);
    assert_eq!(extract.functions.len(), 1);
    assert_eq!(extract.functions[0].name, "clamp");
    assert!(!extract.functions[0].requires.is_empty());
    assert!(!extract.functions[0].ensures.is_empty());
}

#[test]
fn verified_true_java_routes_to_openjml_not_dafny() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "java-verified";
    write_java_project(root, change, true);

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some(change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });

    let raw = fs::read_to_string(verify_report_path(root, change)).expect("verify report");
    assert!(
        !raw.contains("lsc check"),
        "java should not invoke LemmaScript: {raw}"
    );
    let proofs_ran = raw.contains("\"proofs\": \"pass\"")
        || raw.contains("\"proofs\": \"fail\"")
        || raw.contains("openjml -esc")
        || raw.contains("OpenJML proof engine not available")
        || raw.contains("proof check failed");
    assert!(
        proofs_ran,
        "expected proofs phase to run for verified java: {raw}"
    );
    let _ = code;
}

#[test]
fn verified_false_java_skips_proof_phase_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "java-spec-only";
    write_java_project(root, change, false);

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some(change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    assert_eq!(code, 0);

    let raw = fs::read_to_string(verify_report_path(root, change)).expect("verify report");
    assert!(
        !raw.contains("proof check failed"),
        "spec-tracked java should not run OpenJML proofs: {raw}"
    );
}

#[test]
fn ensure_java_patches_legacy_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_project(root);
    let config_path = forall_workflow::paths::verify_adapters_path(root);
    fs::write(
        &config_path,
        "backend: dafny\nstrict: false\ncli_js: \"/tmp/forall\"\nadapters:\n  typescript:\n    proof: lemmascript-dafny\n    extensions: [ts, tsx]\n  python:\n    proof: none\n    extensions: [py]\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/Main.java"), "public class Main {}\n").unwrap();

    assert!(ensure_java_verification(root));
    let raw = fs::read_to_string(&config_path).unwrap();
    assert!(raw.contains("openjml"));
}

#[test]
fn ensure_java_adapter_persisted_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    assert!(!ensure_java_adapter_persisted(dir.path()).unwrap());
    let config = load_verified_config(dir.path());
    assert_eq!(
        config.adapter_for_file("src/Foo.java").unwrap().1.proof,
        PROOF_OPENJML
    );
}
