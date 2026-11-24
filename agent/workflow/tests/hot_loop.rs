//! Hot loop, PBT counterexample, session snapshot, and tier-claim gates.

#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use forall_workflow::build_workflow_status;
use forall_workflow::commands::archive::{ArchiveOptions, run_archive};
use forall_workflow::commands::check::{CheckOptions, run_check};
use forall_workflow::hooks::browser_claims::{
    BrowserClaimInput, validate_browser_verification_claims,
};
use forall_workflow::hooks::stop_success::{build_stop_check_success_message, stop_tier_line};
use forall_workflow::hooks::tier_claims::{TierClaimInput, validate_tier_honesty_claims};
use forall_workflow::mapping::schema::{IssueSeverity, VerificationSummary};
use forall_workflow::verify::report::read_verify_report;

mod support;
use support::{
    change_dir, init_project, node_available, verify_report_path, write_active_change,
    write_clamp_project, write_spec_tracked_project,
};

fn property_test_issue(
    report: &forall_workflow::mapping::schema::VerifyReport,
) -> &forall_workflow::mapping::schema::VerifyIssue {
    report
        .issues
        .iter()
        .find(|i| i.phase == "property-tests" && i.severity == IssueSeverity::Critical)
        .expect("expected property-tests CRITICAL issue")
}

#[test]
fn broken_clamp_emits_counterexample_and_seed() {
    if !node_available() {
        eprintln!("skip broken_clamp: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", true);

    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some("add-clamp"),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    assert_ne!(code, 0, "broken clamp should fail check");

    let report = read_verify_report(root, Some("add-clamp")).expect("verify report");
    assert!(!report.ok);
    assert_eq!(
        report.phases.get("property-tests"),
        Some(&forall_workflow::mapping::schema::PhaseStatus::Fail)
    );

    let issue = property_test_issue(&report);
    let ce = issue
        .counterexample
        .as_ref()
        .expect("counterexample in report");
    assert!(ce.get("x").is_some(), "counterexample should include x");
    assert!(ce.get("lo").is_some(), "counterexample should include lo");
    assert!(ce.get("hi").is_some(), "counterexample should include hi");

    let pbt = issue.pbt.as_ref().expect("pbt metadata");
    assert_eq!(pbt.seed, 42);

    let json_raw = fs::read_to_string(verify_report_path(root, "add-clamp")).expect("report json");
    assert!(
        json_raw.contains("\"requirementId\""),
        "report v2 should serialize requirementId in camelCase"
    );
    assert!(
        json_raw.contains("\"examplesRun\""),
        "report v2 should serialize examplesRun in camelCase"
    );
}

#[test]
fn pbt_seed_reproduces_failure_and_fix_passes() {
    if !node_available() {
        eprintln!("skip pbt_seed: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", true);

    let check_opts = || CheckOptions {
        root: Some(root),
        change: Some("add-clamp"),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    };

    assert_ne!(run_check(check_opts()), 0);
    let first = read_verify_report(root, Some("add-clamp")).expect("first report");
    let first_ce = property_test_issue(&first)
        .counterexample
        .clone()
        .expect("first counterexample");

    assert_ne!(run_check(check_opts()), 0);
    let second = read_verify_report(root, Some("add-clamp")).expect("second report");
    let second_ce = property_test_issue(&second)
        .counterexample
        .clone()
        .expect("second counterexample");
    assert_eq!(
        first_ce, second_ce,
        "same --pbt-seed should reproduce counterexample"
    );

    fs::write(
        root.join("src/clamp.ts"),
        "export function clamp(x: number, lo: number, hi: number): number {\n  if (x < lo) return lo;\n  if (x > hi) return hi;\n  return x;\n}\n",
    )
    .unwrap();
    assert_eq!(run_check(check_opts()), 0, "fixed clamp should pass check");
}

#[test]
fn archive_blocked_on_property_test_critical() {
    if !node_available() {
        eprintln!("skip archive property-test: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", true);

    let archive_code = run_archive(ArchiveOptions {
        name: "add-clamp",
        root: Some(root),
        skip_check: false,
        skip_specs: true,
        waiver_ack: None,
    });
    assert_ne!(
        archive_code, 0,
        "archive should fail when property-tests are CRITICAL"
    );
    assert!(
        change_dir(root, "add-clamp").is_dir(),
        "change should remain active after failed archive"
    );
}

#[test]
fn workflow_status_includes_next_artifact_for_incomplete_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_project(root);
    fs::create_dir_all(change_dir(root, "add-clamp")).unwrap();
    write_active_change(root, "add-clamp");

    let status = build_workflow_status(root, Some("add-clamp"));
    assert_eq!(status.active_change.as_deref(), Some("add-clamp"));
    assert!(!status.apply_ready);
    assert!(
        status.next_artifact.is_some(),
        "session snapshot should include nextArtifact for incomplete change"
    );
    assert!(status.has_forall_project);
}

#[test]
fn check_completes_under_30_seconds() {
    if !node_available() {
        eprintln!("skip check timing: node/npm unavailable");
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_clamp_project(root, "add-clamp", false);

    let start = Instant::now();
    let code = run_check(CheckOptions {
        root: Some(root),
        change: Some("add-clamp"),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });
    let elapsed = start.elapsed();

    assert_eq!(code, 0, "check should pass on fixed clamp");
    assert!(
        elapsed < Duration::from_secs(30),
        "M1-B: check took {elapsed:?}, expected < 30s"
    );
    eprintln!("M1-B baseline: check completed in {elapsed:?}");
}

#[test]
fn stop_tier_line_for_spec_tracked_only() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_spec_tracked_project(root, "add-clamp");

    let _ = run_check(CheckOptions {
        root: Some(root),
        change: Some("add-clamp"),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    });

    let report = read_verify_report(root, Some("add-clamp")).expect("verify report");
    assert!(report.ok);

    let tier = stop_tier_line(&report.verification_summary);
    assert!(
        tier.contains("0 proved"),
        "expected 0 proved in tier line, got: {tier}"
    );

    let msg = build_stop_check_success_message("add-clamp", &report, Some(&tier)).unwrap();
    assert!(msg.contains("0 proved"));
    assert!(msg.contains("formal verification"));
}

#[test]
fn blocks_formal_verification_claim_with_zero_proved() {
    let summary = VerificationSummary {
        total_requirements: 1,
        proved_requirements: 0,
        property_tested_requirements: 1,
        spec_tracked_requirements: 0,
    };

    let reason = validate_tier_honesty_claims(&TierClaimInput {
        last_assistant_message: Some("The clamp function is formally verified."),
        summary: Some(&summary),
    });
    assert!(
        reason.is_some(),
        "should block over-claiming formal verification"
    );
}

#[test]
fn browser_claims_gate_blocks_without_evidence() {
    let reason = validate_browser_verification_claims(&BrowserClaimInput {
        last_assistant_message: Some("Verified the login page in the browser and UI pass."),
        session_id: Some("sess-m1c"),
        transcript_path: None,
        project_root: Some(Path::new("/tmp/unused")),
    });
    assert!(
        reason.is_some(),
        "browser claims should block without evidence"
    );
}
