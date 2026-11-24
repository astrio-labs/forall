//! Daemon v1 — filesystem watch and debounced incremental check.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::daemon::{daemon_status_path, read_daemon_status, run_daemon_tick};
use forall_workflow::paths::scenarios_dir;

mod support;
use support::{change_dir, init_project, write_active_change};

fn write_clamp_project(root: &Path, change: &str, broken: bool) {
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
        include_str!("../schemas/forall/templates/clamp-bounds.property.ts"),
    )
    .unwrap();

    fs::write(
        root.join("package.json"),
        r#"{"private":true,"type":"module","dependencies":{"fast-check":"^3.23.2"}}"#,
    )
    .unwrap();
    let npm = std::process::Command::new("npm")
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

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
        && std::process::Command::new("npm")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

#[test]
fn daemon_tick_writes_daemon_status_json() {
    if !node_available() {
        eprintln!("skip daemon tick: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", true);

    let snapshot = run_daemon_tick(root, Some("add-clamp"), &[root.join("src/clamp.ts")])
        .expect("daemon tick");

    assert!(daemon_status_path(root).is_file());
    let loaded = read_daemon_status(root).expect("read daemon status");
    assert_eq!(loaded.last_check_ok, snapshot.last_check_ok);
    assert_eq!(loaded.active_change.as_deref(), Some("add-clamp"));
    assert!(!loaded.checking);
    assert_eq!(loaded.last_check_ok, Some(false));
    assert!(
        loaded.failed_phases.iter().any(|p| p == "property-tests"),
        "expected property-tests failure, got {:?}",
        loaded.failed_phases
    );
    assert!(loaded.top_issue.is_some());
}

#[test]
fn daemon_tick_updates_within_debounce_window() {
    if !node_available() {
        eprintln!("skip daemon debounce: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", false);

    let start = Instant::now();
    run_daemon_tick(root, Some("add-clamp"), &[root.join("src/clamp.ts")]).expect("daemon tick");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "daemon tick took {elapsed:?}, expected < 30s"
    );
    let loaded = read_daemon_status(root).expect("daemon status");
    assert_eq!(loaded.last_check_ok, Some(true));
}

#[test]
fn daemon_once_full_check_via_empty_paths() {
    if !node_available() {
        eprintln!("skip daemon once: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", false);

    let snapshot = run_daemon_tick(root, Some("add-clamp"), &[]).expect("full daemon tick");
    assert_eq!(snapshot.last_check_ok, Some(true));

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some("add-clamp"),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    assert_eq!(code, 0);
}
