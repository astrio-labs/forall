//! Wire-contract conformance: the fixtures under tests/fixtures/wire/ are
//! byte-identical copies of the canonical server fixtures in the private repo
//! (mcp-host/tests/fixtures/wire/, exercised there against the server's own
//! DTOs with a strict round-trip). This side proves the hand-mirrored client
//! DTOs still parse exactly what the server emits. The wire types exist in
//! three hand-synced Rust copies plus a Python lambda; until they share a
//! crate, these fixtures are what stops silent drift. Change a wire type or
//! fixture on either side and you must update the twin in the same change.
//!
//! Client types deliberately do NOT serialize back to byte-identical JSON
//! (absent optionals become explicit nulls here), so equality is asserted
//! structurally: every field the fixture carries must survive the round trip
//! with the same value. Fields the client drops or renames therefore fail
//! here even when no assertion below happens to name them.

use std::path::Path;

use serde_json::Value;

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

/// Every key the fixture has, at every depth, must be present in the client's
/// re-serialization with an equal value. Extra keys on the client side (the
/// null-filled optionals) are allowed; missing or changed ones are not — that
/// is the drift this suite exists to catch.
fn assert_covers(fixture: &Value, client: &Value, path: &str) {
    match fixture {
        Value::Object(expected) => {
            let actual = client
                .as_object()
                .unwrap_or_else(|| panic!("{path}: client dropped object"));
            for (key, value) in expected {
                let found = actual
                    .get(key)
                    .unwrap_or_else(|| panic!("{path}/{key}: field missing after round trip"));
                assert_covers(value, found, &format!("{path}/{key}"));
            }
        }
        Value::Array(expected) => {
            let actual = client
                .as_array()
                .unwrap_or_else(|| panic!("{path}: client dropped array"));
            assert_eq!(expected.len(), actual.len(), "{path}: array length changed");
            for (index, value) in expected.iter().enumerate() {
                assert_covers(value, &actual[index], &format!("{path}[{index}]"));
            }
        }
        scalar => assert_eq!(scalar, client, "{path}: value changed after round trip"),
    }
}

fn parse(name: &str) -> StatusVerificationResponse {
    let raw = fixture(name);
    let parsed: StatusVerificationResponse =
        serde_json::from_str(&raw).expect("server fixture parses with client DTOs");
    let reserialized = serde_json::to_value(&parsed).expect("client DTO serializes");
    let original: Value = serde_json::from_str(&raw).expect("fixture is JSON");
    assert_covers(&original, &reserialized, name);
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
    let ledger = result
        .ledger
        .as_ref()
        .expect("ledger rides inside the report");
    assert_eq!(ledger.entries.len(), 1);
    assert_eq!(ledger.entries[0].requirement_id, "REQ-007");
    assert_eq!(ledger.entries[0].claimed_level, "proved");
    assert_eq!(ledger.entries[0].earned_level, "contracted");
    assert_eq!(
        ledger.entries[0].obligations[0].tool_version.as_deref(),
        Some("Frama-C 32.1 (Iron)")
    );
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
