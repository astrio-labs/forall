//! Brownfield mapping import from existing //@ annotations.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;

use forall_workflow::commands::mapping_import::{
    MappingImportCommandOptions, run_mapping_import_command,
};
use forall_workflow::commands::workflow::run_init;
use forall_workflow::mapping::load::load_project_mapping;
use forall_workflow::mapping::scan::discover_typescript_files;

fn lsc_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn write_annotated_clamp_project(root: &Path) {
    run_init(root, false, false, false);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/clamp.ts"),
        "export function clamp(x: number, lo: number, hi: number): number {\n  //@ requires lo <= hi\n  //@ ensures lo <= $result && $result <= hi\n  if (x < lo) return lo;\n  if (x > hi) return hi;\n  return x;\n}\n",
    )
    .unwrap();
}

#[test]
fn discovers_typescript_files_under_src() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    run_init(root, false, false, false);
    fs::create_dir_all(root.join("src/math")).unwrap();
    fs::write(root.join("src/math/a.ts"), "export const a = 1;\n").unwrap();
    fs::write(root.join("src/math/b.tsx"), "export const b = 1;\n").unwrap();

    let files = discover_typescript_files(root);
    assert!(files.contains(&"src/math/a.ts".to_string()));
    assert!(files.contains(&"src/math/b.tsx".to_string()));
}

#[test]
fn import_adds_requirements_for_annotated_symbols() {
    if !lsc_available() {
        eprintln!("skip mapping import integration: node unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_annotated_clamp_project(root);

    assert_eq!(
        run_mapping_import_command(MappingImportCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );

    let mapping = load_project_mapping(root).expect("mapping");
    let imported = mapping
        .requirements
        .iter()
        .find(|req| {
            req.code
                .as_ref()
                .is_some_and(|code| code.file == "src/clamp.ts" && code.symbols == ["clamp"])
        })
        .expect("clamp requirement");
    assert!(imported.verified);
}

#[test]
fn dry_run_does_not_modify_mapping_file() {
    if !lsc_available() {
        eprintln!("skip mapping import dry run: node unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_annotated_clamp_project(root);
    let mapping_path = forall_workflow::paths::forall_mapping_path(root);
    let before = fs::read_to_string(&mapping_path).unwrap();

    assert_eq!(
        run_mapping_import_command(MappingImportCommandOptions {
            root: Some(root),
            dry_run: true,
            json: false,
        }),
        0
    );

    let after = fs::read_to_string(&mapping_path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn second_import_skips_existing_symbols() {
    if !lsc_available() {
        eprintln!("skip mapping import merge: node unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_annotated_clamp_project(root);

    assert_eq!(
        run_mapping_import_command(MappingImportCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );
    let first_count = load_project_mapping(root).unwrap().requirements.len();

    assert_eq!(
        run_mapping_import_command(MappingImportCommandOptions {
            root: Some(root),
            dry_run: false,
            json: false,
        }),
        0
    );
    let second_count = load_project_mapping(root).unwrap().requirements.len();
    assert_eq!(first_count, second_count);
}
