//! Wire-contract conformance: the fixtures under tests/fixtures/wire/ are
//! byte-identical copies of the canonical server fixtures in the private repo
//! (mcp-host/tests/fixtures/wire/, exercised there against the server's own
//! DTOs with a strict round-trip). This side proves the hand-mirrored client
//! DTOs still parse exactly what the server emits. The wire types exist in
//! three hand-synced Rust copies plus a Python lambda; until they share a
//! crate, these fixtures are what stops silent drift. Change a wire type or
//! fixture on either side and you must update the twin in the same change.
//!
//! Client types deliberately do NOT round-trip to identical JSON (absent
//! optionals serialize as explicit nulls here), so this side asserts parsed
//! values and re-parse stability, not byte equality.

use std::path::Path;

use forall_hosted_verify::{
    StatusVerificationResponse, VerificationIssueSeverity, VerificationPhaseStatus,
    VerificationStatus,
};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wire")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn parse(name: &str) -> StatusVerificationResponse {
    let raw = fixture(name);
    let parsed: StatusVerificationResponse =
        serde_json::from_str(&raw).expect("server fixture parses with client DTOs");
    // Re-serialize and re-parse: whatever the client emits must at least be
    // stable under its own parser, or downstream caching breaks.
    let reserialized = serde_json::to_string(&parsed).expect("client DTO serializes");
    let reparsed: StatusVerificationResponse =
        serde_json::from_str(&reserialized).expect("client DTO re-parses its own output");
    assert_eq!(parsed, reparsed, "{name}: client DTO not self-stable");
    parsed
}

#[test]
fn succeeded_status_parses_with_the_full_report() {
    let response = parse("succeeded_status.json");

    assert_eq!(response.contract_version, 1);
    assert_eq!(response.status, VerificationStatus::Succeeded);
    assert_eq!(
        response.source_revision.as_deref(),
        Some("4f2c9c3c1a0b7d6e5f4a3b2c1d0e9f8a7b6c5d4e")
    );
    let result = response.result.expect("succeeded job carries a report");
    assert!(!result.ok);
    assert_eq!(
        result.phases.get("proofs"),
        Some(&VerificationPhaseStatus::Fail)
    );
    assert_eq!(result.issues.len(), 3);
    let critical = &result.issues[0];
    assert_eq!(critical.severity, VerificationIssueSeverity::Critical);
    assert_eq!(critical.requirement_id.as_deref(), Some("REQ-007"));
    assert!(
        critical
            .proof_detail
            .as_deref()
            .is_some_and(|detail| detail.starts_with("tool: ")),
        "proofs-phase detail leads with the prover identity line"
    );
    assert!(result.issues[1].counterexample.is_some());
    assert!(result.issues[2].requirement_id.is_none());
    assert_eq!(result.verification_summary.total_requirements, 9);
    assert_eq!(result.verification_summary.proved_requirements, 4);
}

#[test]
fn running_status_parses_with_progress_only() {
    let response = parse("running_status.json");
    assert_eq!(response.status, VerificationStatus::Running);
    assert!(response.result.is_none());
    assert!(response.error.is_none());
    let progress = response.progress.expect("running job reports progress");
    assert_eq!((progress.completed, progress.total), (3, 7));
}

#[test]
fn failed_status_parses_with_a_retryable_error() {
    let response = parse("failed_status.json");
    assert_eq!(response.status, VerificationStatus::Failed);
    assert!(response.result.is_none());
    let error = response.error.expect("failed job carries an error");
    assert!(error.retryable);
    assert_eq!(error.code, "worker_crashed");
}
