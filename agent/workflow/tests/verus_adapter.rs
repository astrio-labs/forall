//! Verus adapter registry and contract extraction tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::commands::workflow::{ensure_rust_verification, run_init};
use forall_workflow::verify::adapters::{
    PROOF_VERUS, ensure_rust_adapter_persisted, load_verified_config,
};
use forall_workflow::verify::backends::verus::extract_verus_contracts;

mod support;
use support::{change_dir, init_project, verify_report_path, write_active_change};

fn write_rust_project(root: &std::path::Path, change: &str, verified: bool) {
    init_project(root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[package.metadata.verus]\nverify = true\n",
    )
    .unwrap();
    fs::write(
        root.join("src/clamp.rs"),
        r#"pub fn clamp(x: u64, lo: u64, hi: u64) -> u64
    requires lo <= hi,
    ensures lo <= result <= hi,
{
    if x < lo { lo } else if x > hi { hi } else { x }
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
      file: src/clamp.rs
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
fn default_config_registers_verus_for_rust() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let config = load_verified_config(dir.path());
    let (_, adapter) = config.adapter_for_file("src/foo.rs").unwrap();
    assert_eq!(adapter.proof, PROOF_VERUS);
}

#[test]
fn verus_extractor_finds_requires_and_ensures() {
    let source = r#"
pub fn clamp(x: u64, lo: u64, hi: u64) -> (result: u64)
    requires lo <= hi,
    ensures lo <= result <= hi,
{
    x
}
"#;
    let extract = extract_verus_contracts(source);
    assert_eq!(extract.functions.len(), 1);
    assert_eq!(extract.functions[0].name, "clamp");
    assert!(!extract.functions[0].requires.is_empty());
    assert!(!extract.functions[0].ensures.is_empty());
}

#[test]
fn verified_true_rust_routes_to_verus_not_dafny() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "rust-verified";
    write_rust_project(root, change, true);

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
        "rust should not invoke LemmaScript: {raw}"
    );
    assert!(
        raw.contains("cargo verus verify")
            || raw.contains("proof check failed")
            || raw.contains("Verus proof engine not available"),
        "expected Verus routing evidence in report: {raw}"
    );
    let _ = code;
}

#[test]
fn verified_false_rust_skips_proof_phase_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "rust-spec-only";
    write_rust_project(root, change, false);

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
        "spec-tracked rust should not run Verus proofs: {raw}"
    );
}

#[test]
fn init_rust_scaffold_writes_cargo_and_sample_mapping() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert_eq!(run_init(root, true, false, false), 0);
    assert!(root.join("Cargo.toml").is_file());
    assert!(root.join("src/lib.rs").is_file());
    assert!(
        forall_workflow::paths::verify_dir(root)
            .join("mapping.sample.yaml")
            .is_file()
    );
}

#[test]
fn ensure_rust_patches_legacy_config_and_cargo_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_init(root, false, false, false);
    let config_path = forall_workflow::paths::verify_adapters_path(root);
    fs::write(
        &config_path,
        "backend: dafny\nstrict: false\ncli_js: \"/tmp/forall\"\nadapters:\n  typescript:\n    proof: lemmascript-dafny\n    extensions: [ts, tsx]\n  python:\n    proof: none\n    extensions: [py]\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"todo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();

    let result = ensure_rust_verification(root, false);
    assert!(result.config_updated);
    assert_eq!(result.cargo_updated, vec!["Cargo.toml".to_string()]);
    let raw = fs::read_to_string(&config_path).unwrap();
    assert!(raw.contains("verus"));
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("[package.metadata.verus]"));
}

#[test]
fn ensure_rust_adapter_persisted_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    assert!(!ensure_rust_adapter_persisted(dir.path()).unwrap());
    let config = load_verified_config(dir.path());
    assert_eq!(
        config.adapter_for_file("src/foo.rs").unwrap().1.proof,
        PROOF_VERUS
    );
}
