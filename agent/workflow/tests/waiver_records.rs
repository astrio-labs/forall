//! Explicit waiver records — audited archive despite waivable CRITICAL issues.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;

use forall_workflow::commands::archive::{ArchiveOptions, run_archive};
use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::commands::waive::{WaiveOptions, run_waive};
use forall_workflow::waiver::{WAIVERS_REL, find_waiver_covering, waivable_critical_ids};

mod support;
use support::{change_dir, init_project, node_available, write_active_change, write_clamp_project};

fn write_waivable_mapping_critical_project(root: &Path, change: &str) {
    init_project(root);
    fs::create_dir_all(change_dir(root, change)).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/demo.py"), "def demo():\n    return 1\n").unwrap();
    write_active_change(root, change);
    fs::write(
        change_dir(root, change).join("mapping.delta.yaml"),
        r#"version: 1
requirements:
  - id: py-demo
    capability: demo
    requirement: demo behavior
    verified: true
    code:
      file: src/demo.py
      symbols: [demo]
"#,
    )
    .unwrap();
}

#[test]
fn waive_records_json_for_mapping_critical() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "waive-mapping";
    write_waivable_mapping_critical_project(root, change);

    assert_ne!(
        run_check(CheckOptions {
            root: Some(root),
            change: Some(change),
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        }),
        0
    );

    assert_eq!(
        run_waive(WaiveOptions {
            root: Some(root),
            change,
            ack: "User confirmed archive despite incomplete mapping.",
            json: false,
        }),
        0
    );

    let waivers_dir = root.join(WAIVERS_REL);
    let entries: Vec<_> = fs::read_dir(&waivers_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1);
    let raw = fs::read_to_string(&entries[0]).unwrap();
    assert!(raw.contains("User confirmed archive despite incomplete mapping."));
    assert!(raw.contains("mapping"));
}

#[test]
fn archive_with_ack_proceeds_on_waivable_critical() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "archive-with-ack";
    write_waivable_mapping_critical_project(root, change);

    assert_ne!(
        run_archive(ArchiveOptions {
            name: change,
            root: Some(root),
            skip_check: false,
            skip_specs: true,
            waiver_ack: None,
        }),
        0,
        "archive should fail without waiver"
    );
    assert!(
        change_dir(root, change).is_dir(),
        "change should remain active"
    );

    assert_eq!(
        run_archive(ArchiveOptions {
            name: change,
            root: Some(root),
            skip_check: false,
            skip_specs: true,
            waiver_ack: Some("User explicitly waived mapping CRITICAL."),
        }),
        0,
        "archive should succeed with --ack"
    );
    assert!(
        !change_dir(root, change).is_dir(),
        "change should be archived"
    );
}

#[test]
fn archive_reuses_existing_waiver() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let change = "reuse-waiver";
    write_waivable_mapping_critical_project(root, change);

    let (report, _, _, _) = forall_workflow::commands::check::run_check_report(CheckOptions {
        root: Some(root),
        change: Some(change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    })
    .unwrap();
    let ids = waivable_critical_ids(&report);
    assert_eq!(
        run_waive(WaiveOptions {
            root: Some(root),
            change,
            ack: "User confirmed once.",
            json: false,
        }),
        0
    );
    assert!(find_waiver_covering(root, change, &ids).is_some());

    assert_eq!(
        run_archive(ArchiveOptions {
            name: change,
            root: Some(root),
            skip_check: false,
            skip_specs: true,
            waiver_ack: None,
        }),
        0,
        "archive should reuse recorded waiver"
    );
}

#[test]
fn property_test_critical_cannot_be_waived() {
    if !node_available() {
        eprintln!("skip property_test waiver: node unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", true);

    assert_ne!(
        run_archive(ArchiveOptions {
            name: "add-clamp",
            root: Some(root),
            skip_check: false,
            skip_specs: true,
            waiver_ack: Some("User tried to waive property-test failure."),
        }),
        0,
        "property-test CRITICAL must not be waivable"
    );
}
