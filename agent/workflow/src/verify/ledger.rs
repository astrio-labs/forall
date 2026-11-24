//! The obligation ledger: what a check run actually PROVED, per requirement,
//! with the toolchain identity behind each verdict.
//!
//! The mapping states intent; this file records outcomes. The separation is
//! the point — the editor's citation surface derives every "proved" pixel
//! from ledger entries written by `forall check`, never from the mapping's
//! agent-writable claims. Every phase that reports structured per-requirement
//! results contributes evidence: proofs, seeded property runs and scenario
//! runs, each tagged with its [`EvidenceKind`] so grading can tell a proof
//! from statistical evidence from an example-based one. Per-symbol AST
//! fingerprints replace `file_sha256` when annotation-aware hashing lands.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mapping::schema::{CodeRef, EvidenceLevel, Requirement, claimed_evidence_level};
use crate::paths::{verify_reports_dir, verify_reports_read_dir};
use crate::verify::fingerprint::{FingerprintLanguage, symbol_fingerprints};

pub const LEDGER_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationStatus {
    Proved,
    Failed,
    /// The prover was not available; distinct from a failed proof so a gap in
    /// tooling never reads as a developer failure.
    ToolUnavailable,
}

/// What KIND of evidence discharged an obligation. The distinction is
/// load-bearing: a property run is statistical, so it may earn
/// PropertyTested and must never earn Proved no matter how many examples
/// passed. Defaults to Proof so ledgers written before the split still
/// read correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    #[default]
    Proof,
    PropertyTest,
    /// A scenario test that ran and passed. Example-based, so it ranks below
    /// a property run: a scenario checks the cases someone thought of, a
    /// property run checks cases nobody thought of.
    ScenarioTest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObligationRecord {
    #[serde(default)]
    pub kind: EvidenceKind,
    /// Human summary of what this obligation asserts.
    pub description: String,
    /// Source anchor: the mapped file. Symbol-level anchors arrive with
    /// AST-aware hashing; consumers must treat this as file-granular.
    pub file: String,
    /// The symbols the requirement cites in that file.
    pub symbols: Vec<String>,
    pub status: ObligationStatus,
    pub tool: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub requirement_id: String,
    /// The requirement's code citation at check time. Carried on the entry
    /// itself so the citation surface can anchor EVERY entry to a file —
    /// including contracted-but-unproven ones with no obligations — without
    /// joining back through the agent-writable mapping the ledger exists to
    /// bypass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<CodeRef>,
    /// The rung the mapping CLAIMS (intent, agent-writable in v1 mappings).
    pub claimed_level: EvidenceLevel,
    /// The rung this run's machine evidence actually supports: the highest
    /// of Proved from the proofs phase, Contracted from an attached contract,
    /// PropertyTested from a seeded property run, Tested from a scenario run,
    /// and SpecTracked from a code mapping. Highest rather than first-match,
    /// so gaining evidence can never lower the rung.
    pub earned_level: EvidenceLevel,
    /// SHA-256 of the cited file's bytes at check time. The coarse fallback
    /// for symbols the fingerprinter cannot resolve; per-symbol fingerprints
    /// below are the precise signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_sha256: Option<String>,
    /// Annotation-aware fingerprints of each cited symbol at check time,
    /// the staleness engine's comparison baseline. Symbols the fingerprinter
    /// cannot resolve are absent — absence reads as "unknown", never
    /// "unchanged".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbol_fingerprints: Vec<SymbolFingerprintRecord>,
    /// SHA-256 of the requirement's normative text at check time; a changed
    /// requirement sentence is spec drift even when the code stands still.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub requirement_sha256: String,
    pub obligations: Vec<ObligationRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolFingerprintRecord {
    pub symbol: String,
    /// Code plus formal annotations.
    pub full: String,
    /// Code only — unchanged `code` with changed `full` is an
    /// annotation-only edit, i.e. spec drift rather than code drift.
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ledger {
    pub version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    pub entries: Vec<LedgerEntry>,
    /// Tamper-evident seal over the entries, verified by `read_ledger`,
    /// which fails CLOSED on mismatch: a hand-edited ledger yields no
    /// evidence at all rather than forged evidence. This is integrity
    /// against casual and accidental edits, not a cryptographic boundary —
    /// the algorithm is public, so a deliberate forger can re-seal.
    /// Unforgeable signatures arrive with hosted certificate signing; until
    /// then this plus the pre-tool-use gate is the defense in depth.
    #[serde(default)]
    pub integrity: String,
}

fn integrity_seal(version: u32, change: Option<&str>, entries: &[LedgerEntry]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"forall-ledger-seal-v1\0");
    hasher.update(version.to_le_bytes());
    if let Some(change) = change {
        hasher.update(change.as_bytes());
    }
    hasher.update(b"\0");
    // Struct-order serde serialization is deterministic for a given schema
    // version, which is all the seal needs.
    hasher.update(
        serde_json::to_string(entries)
            .unwrap_or_default()
            .as_bytes(),
    );
    format!("{:x}", hasher.finalize())
}

/// One property-phase outcome, collected per requirement. Recorded even
/// when the run PASSES — a silent success is why the ledger could never
/// grant PropertyTested before.
#[derive(Debug, Clone)]
pub struct PropertyOutcome {
    pub requirement_id: String,
    pub file: String,
    pub status: ObligationStatus,
    pub seed: u64,
    pub examples_run: u32,
}

/// One scenario test that actually ran, and how it went.
///
/// The scenarios phase used to report only failures, so a passing scenario
/// left no trace and `Tested` (E2) was a rung nothing could ever earn. An
/// outcome per run — pass included — is what makes the rung reachable, and it
/// keeps the ledger's rule that a rung is earned from a recorded run rather
/// than inferred from the absence of a complaint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioOutcome {
    pub requirement_id: String,
    pub file: String,
    pub status: ObligationStatus,
}

/// One proofs-phase outcome for a mapped file, collected by `execute_check`
/// while dispatching backends.
#[derive(Debug, Clone)]
pub struct ProofOutcome {
    pub file: String,
    pub tool: String,
    pub tool_version: Option<String>,
    pub status: ObligationStatus,
}

/// Build ledger entries by joining proofs-phase outcomes back onto the
/// requirements. Every requirement gets an entry — a requirement with no
/// proof outcome still records its claimed level and an earned level derived
/// from artifacts that exist (contract, code mapping), so the citation
/// surface can render the whole mapping from the ledger alone.
pub fn build_ledger(
    root: &Path,
    change: Option<&str>,
    requirements: &[Requirement],
    outcomes: &[ProofOutcome],
) -> Ledger {
    build_ledger_with_properties(root, change, requirements, outcomes, &[])
}

pub fn build_ledger_with_properties(
    root: &Path,
    change: Option<&str>,
    requirements: &[Requirement],
    outcomes: &[ProofOutcome],
    property_outcomes: &[PropertyOutcome],
) -> Ledger {
    build_ledger_with_evidence(root, change, requirements, outcomes, property_outcomes, &[])
}

/// Build ledger entries from every phase that reports per-requirement
/// outcomes: proofs, property runs and scenario runs.
pub fn build_ledger_with_evidence(
    root: &Path,
    change: Option<&str>,
    requirements: &[Requirement],
    outcomes: &[ProofOutcome],
    property_outcomes: &[PropertyOutcome],
    scenario_outcomes: &[ScenarioOutcome],
) -> Ledger {
    // What a requirement CLAIMS is read differently per mapping version, and
    // the version is a property of the project we are already rooted in —
    // deriving it here keeps it off every caller's signature.
    let version = crate::mapping::load::mapping_version_for_check(root, change);
    let by_file: BTreeMap<&str, Vec<&ProofOutcome>> =
        outcomes.iter().fold(BTreeMap::new(), |mut map, outcome| {
            map.entry(outcome.file.as_str()).or_default().push(outcome);
            map
        });

    let entries = requirements
        .iter()
        .map(|req| {
            let claimed = claimed_evidence_level(req, version);
            let symbol_fingerprints = req
                .code
                .as_ref()
                .map(|code| compute_symbol_fingerprints(root, code))
                .unwrap_or_default();
            let (obligations, file_sha256) = match &req.code {
                Some(code) => {
                    let file_outcomes = by_file.get(code.file.as_str());
                    let obligations = file_outcomes
                        .map(|file_outcomes| {
                            file_outcomes
                                .iter()
                                .map(|outcome| ObligationRecord {
                                    kind: EvidenceKind::Proof,
                                    description: format!(
                                        "formal proof obligations for {} discharged by {}",
                                        code.file, outcome.tool
                                    ),
                                    file: code.file.clone(),
                                    symbols: code.symbols.clone(),
                                    status: outcome.status,
                                    tool: outcome.tool.clone(),
                                    tool_version: outcome.tool_version.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (obligations, file_sha256(root, &code.file))
                }
                None => (Vec::new(), None),
            };

            // Property outcomes join the same obligation list, tagged so
            // grading can tell statistical evidence from a proof.
            let mut obligations = obligations;
            obligations.extend(
                property_outcomes
                    .iter()
                    .filter(|outcome| outcome.requirement_id == req.id)
                    .map(|outcome| ObligationRecord {
                        kind: EvidenceKind::PropertyTest,
                        description: format!(
                            "property run over {} examples (seed {})",
                            outcome.examples_run, outcome.seed
                        ),
                        file: outcome.file.clone(),
                        symbols: req
                            .code
                            .as_ref()
                            .map(|code| code.symbols.clone())
                            .unwrap_or_default(),
                        status: outcome.status,
                        tool: "property-test".to_string(),
                        tool_version: None,
                    }),
            );

            // Scenario outcomes join the same list, tagged so grading can tell
            // an example-based run from a seeded property run from a proof.
            obligations.extend(
                scenario_outcomes
                    .iter()
                    .filter(|outcome| outcome.requirement_id == req.id)
                    .map(|outcome| ObligationRecord {
                        kind: EvidenceKind::ScenarioTest,
                        description: format!("scenario test {}", outcome.file),
                        file: outcome.file.clone(),
                        symbols: req
                            .code
                            .as_ref()
                            .map(|code| code.symbols.clone())
                            .unwrap_or_default(),
                        status: outcome.status,
                        tool: "scenario-test".to_string(),
                        tool_version: None,
                    }),
            );

            let earned = earned_level_for(&obligations, req);

            LedgerEntry {
                requirement_id: req.id.clone(),
                code: req.code.clone(),
                claimed_level: claimed,
                earned_level: earned,
                file_sha256,
                symbol_fingerprints,
                requirement_sha256: requirement_text_sha256(req),
                obligations,
            }
        })
        .collect();

    Ledger {
        version: LEDGER_VERSION,
        change: change.map(str::to_string),
        entries,
        integrity: String::new(),
    }
}

/// The one grading rule, shared by construction and merging so the two can
/// never disagree about what a set of obligations earns.
pub fn earned_level_for(obligations: &[ObligationRecord], req: &Requirement) -> EvidenceLevel {
    let discharged = |kind: EvidenceKind| {
        let mut matching = obligations
            .iter()
            .filter(|obligation| obligation.kind == kind)
            .peekable();
        matching.peek().is_some()
            && obligations
                .iter()
                .filter(|obligation| obligation.kind == kind)
                .all(|obligation| obligation.status == ObligationStatus::Proved)
    };
    // The highest rung the evidence supports, not the first arm that matches.
    //
    // `EvidenceLevel` is ordered by strength, so a first-match chain is only
    // correct if it is written in descending rung order — and it was not.
    // `PropertyTested` (E3) was matched before the `Contracted` (E4) fallback,
    // so a contracted requirement that gained a passing property run *lost* a
    // rung, and the ratchet in `assurance.rs` read that as a regression.
    // Folding with `max` makes the result monotone in evidence by
    // construction: more evidence can never earn less.
    //
    // The ordering itself is unchanged. A contract still outranks a property
    // run, because `Contracted` means machine-checkable intent exists, while
    // a property run is statistical evidence no matter how many examples
    // passed — and neither reaches `Proved`.
    let mut level = EvidenceLevel::None;
    if req.code.is_some() {
        level = level.max(EvidenceLevel::SpecTracked);
    }
    if discharged(EvidenceKind::ScenarioTest) {
        level = level.max(EvidenceLevel::Tested);
    }
    if discharged(EvidenceKind::PropertyTest) {
        level = level.max(EvidenceLevel::PropertyTested);
    }
    if req.contract.is_some() {
        level = level.max(EvidenceLevel::Contracted);
    }
    if discharged(EvidenceKind::Proof) {
        level = level.max(EvidenceLevel::Proved);
    }
    level
}

/// Merge a run's ledger into the existing one, preserving evidence from
/// phases this run did NOT execute.
///
/// A property-only run (the daemon reacting to a changed property file,
/// or a hosted check scoped to PropertyTests) carries no proof outcomes.
/// Rebuilding from it wholesale would erase the proof obligations of the
/// last full run and silently downgrade a Proved requirement to
/// PropertyTested — evidence lost to a narrower run, which is exactly the
/// kind of quiet regression the ledger exists to prevent.
pub fn merge_run(
    existing: Option<Ledger>,
    built: Ledger,
    requirements: &[Requirement],
    ran_proofs: bool,
    ran_properties: bool,
    ran_scenarios: bool,
) -> Ledger {
    let Some(existing) = existing else {
        return built;
    };
    let kept_kind = |kind: EvidenceKind| match kind {
        EvidenceKind::Proof => !ran_proofs,
        EvidenceKind::PropertyTest => !ran_properties,
        EvidenceKind::ScenarioTest => !ran_scenarios,
    };

    let mut entries = built;
    for entry in &mut entries.entries {
        let Some(previous) = existing
            .entries
            .iter()
            .find(|candidate| candidate.requirement_id == entry.requirement_id)
        else {
            continue;
        };
        // An entry has one baseline for all its obligations. Carry a skipped
        // phase forward only if its evidence still belongs to that baseline.
        if previous.requirement_sha256 != entry.requirement_sha256
            || previous.code != entry.code
            || previous.symbol_fingerprints != entry.symbol_fingerprints
            || previous.file_sha256 != entry.file_sha256
            || (entry.code.is_some() && entry.file_sha256.is_none())
        {
            continue;
        }
        entry.obligations.extend(
            previous
                .obligations
                .iter()
                .filter(|obligation| kept_kind(obligation.kind))
                .cloned(),
        );
        if let Some(req) = requirements
            .iter()
            .find(|req| req.id == entry.requirement_id)
        {
            entry.earned_level = earned_level_for(&entry.obligations, req);
        }
    }
    entries
}

/// Fold a partial run's entries into the existing ledger. A requirement-
/// filtered check (the daemon's incremental path) sees only the filtered
/// requirements, so writing its ledger wholesale would erase every
/// unaffected entry until the next full check. Entries from the partial run
/// replace their counterparts in place; everything else is retained. Only a
/// FULL run may prune entries for requirements deleted from the mapping.
pub fn merge_partial(existing: Option<Ledger>, partial: Ledger) -> Ledger {
    let Some(existing) = existing else {
        return partial;
    };
    let mut updated: BTreeMap<&str, &LedgerEntry> = partial
        .entries
        .iter()
        .map(|entry| (entry.requirement_id.as_str(), entry))
        .collect();
    let mut entries: Vec<LedgerEntry> = existing
        .entries
        .iter()
        .map(|entry| {
            updated
                .remove(entry.requirement_id.as_str())
                .cloned()
                .unwrap_or_else(|| entry.clone())
        })
        .collect();
    // Requirements new to the mapping since the last full run.
    entries.extend(
        partial
            .entries
            .iter()
            .filter(|entry| updated.contains_key(entry.requirement_id.as_str()))
            .cloned(),
    );
    Ledger {
        version: partial.version,
        change: partial.change,
        entries,
        integrity: String::new(),
    }
}

fn compute_symbol_fingerprints(
    root: &Path,
    code: &crate::mapping::schema::CodeRef,
) -> Vec<SymbolFingerprintRecord> {
    let Some(language) = FingerprintLanguage::for_path(&code.file) else {
        return Vec::new();
    };
    let Ok(source) = fs::read_to_string(root.join(&code.file)) else {
        return Vec::new();
    };
    code.symbols
        .iter()
        .filter_map(|symbol| {
            symbol_fingerprints(&source, language, symbol).map(|fingerprints| {
                SymbolFingerprintRecord {
                    symbol: symbol.clone(),
                    full: fingerprints.full,
                    code: fingerprints.code,
                }
            })
        })
        .collect()
}

pub fn requirement_text_sha256(requirement: &Requirement) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"forall-requirement-v1\0");
    hasher.update(requirement.requirement.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn file_sha256(root: &Path, relative: &str) -> Option<String> {
    let bytes = fs::read(root.join(relative)).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some(format!("{:x}", hasher.finalize()))
}

/// The report summary for a version-2 mapping, derived from EARNED levels —
/// the ladder's whole point. Totals and counts both come from the ledger's
/// entries, so a filtered run summarized against the merged ledger can never
/// report more proved requirements than total. `fallback_total` covers the
/// no-ledger case only (a fresh v2 project before its first proofs run),
/// where nothing has been machine-checked yet.
pub fn summarize_earned(
    ledger: Option<&Ledger>,
    fallback_total: usize,
) -> crate::mapping::schema::VerificationSummary {
    let Some(ledger) = ledger else {
        return crate::mapping::schema::VerificationSummary {
            total_requirements: fallback_total,
            proved_requirements: 0,
            property_tested_requirements: 0,
            spec_tracked_requirements: fallback_total,
        };
    };
    let total = ledger.entries.len();
    let (proved, property_tested) =
        ledger
            .entries
            .iter()
            .fold((0, 0), |(proved, pbt), entry| match entry.earned_level {
                EvidenceLevel::Proved => (proved + 1, pbt),
                EvidenceLevel::PropertyTested => (proved, pbt + 1),
                _ => (proved, pbt),
            });
    crate::mapping::schema::VerificationSummary {
        total_requirements: total,
        proved_requirements: proved,
        property_tested_requirements: property_tested,
        spec_tracked_requirements: total.saturating_sub(proved + property_tested),
    }
}

pub fn ledger_json_path(root: &Path, change: Option<&str>) -> PathBuf {
    let stem = change.unwrap_or("project");
    verify_reports_read_dir(root).join(format!("{stem}-ledger.json"))
}

/// Written beside the verify report, never through stdout: the specs.check
/// handler parses the subprocess's combined output as JSON, so new artifacts
/// must reach consumers as files.
pub fn write_ledger(root: &Path, ledger: &Ledger) -> std::io::Result<()> {
    let reports_dir = verify_reports_dir(root);
    fs::create_dir_all(&reports_dir)?;
    let stem = ledger.change.as_deref().unwrap_or("project");
    let path = reports_dir.join(format!("{stem}-ledger.json"));
    let mut sealed = ledger.clone();
    sealed.integrity = integrity_seal(sealed.version, sealed.change.as_deref(), &sealed.entries);
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&sealed)?),
    )
}

/// Fails CLOSED on a broken seal: a tampered (or pre-seal legacy) ledger
/// reads as no evidence, never as forged evidence.
pub fn read_ledger(root: &Path, change: Option<&str>) -> Option<Ledger> {
    let raw = fs::read_to_string(ledger_json_path(root, change)).ok()?;
    let ledger: Ledger = serde_json::from_str(&raw).ok()?;
    let expected = integrity_seal(ledger.version, ledger.change.as_deref(), &ledger.entries);
    if ledger.integrity != expected {
        return None;
    }
    Some(ledger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::CodeRef;

    fn requirement(
        id: &str,
        file: Option<&str>,
        contract: Option<&str>,
        verified: bool,
    ) -> Requirement {
        Requirement {
            id: id.to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified,
            property_tested: false,
            property: None,
            code: file.map(|file| CodeRef {
                file: file.to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: contract.map(str::to_string),
            claimcheck: None,
            scenarios: None,
        }
    }

    fn outcome(file: &str, status: ObligationStatus) -> ProofOutcome {
        ProofOutcome {
            file: file.to_string(),
            tool: "framac".to_string(),
            tool_version: Some("Frama-C 32.1".to_string()),
            status,
        }
    }

    #[test]
    fn proved_outcome_earns_e5_and_claims_stay_separate() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/frame.c"),
            b"int f(void) { return 0; }\n",
        )
        .unwrap();

        let requirements = vec![requirement("REQ-1", Some("src/frame.c"), None, true)];
        let outcomes = vec![outcome("src/frame.c", ObligationStatus::Proved)];
        let ledger = build_ledger(dir.path(), None, &requirements, &outcomes);

        let entry = &ledger.entries[0];
        assert_eq!(entry.claimed_level, EvidenceLevel::Proved);
        assert_eq!(entry.earned_level, EvidenceLevel::Proved);
        assert_eq!(entry.obligations.len(), 1);
        assert_eq!(
            entry.obligations[0].tool_version.as_deref(),
            Some("Frama-C 32.1")
        );
        assert_eq!(entry.file_sha256.as_ref().map(String::len), Some(64));
    }

    #[test]
    fn failed_proof_caps_earned_level_below_proved() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![
            requirement("REQ-contracted", Some("src/a.c"), Some("Bounds"), true),
            requirement("REQ-bare", Some("src/a.c"), None, true),
        ];
        let outcomes = vec![outcome("src/a.c", ObligationStatus::Failed)];
        let ledger = build_ledger(dir.path(), Some("change"), &requirements, &outcomes);

        // The mapping still CLAIMS proved; the run earned less. Both facts
        // are recorded, neither overwrites the other.
        assert_eq!(ledger.entries[0].claimed_level, EvidenceLevel::Proved);
        assert_eq!(ledger.entries[0].earned_level, EvidenceLevel::Contracted);
        assert_eq!(ledger.entries[1].earned_level, EvidenceLevel::SpecTracked);
        assert_eq!(
            ledger.entries[0].obligations[0].status,
            ObligationStatus::Failed
        );
        // The cited file does not exist in this root: no hash, not a fake one.
        assert_eq!(ledger.entries[0].file_sha256, None);
    }

    #[test]
    fn contracted_entry_without_outcomes_still_anchors_to_its_file() {
        // The citation surface renders from the ledger alone: an entry with
        // no proof outcomes must still say WHERE its citation lives.
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement(
            "REQ-contracted",
            Some("src/dose.c"),
            Some("Bounds"),
            false,
        )];
        let ledger = build_ledger(dir.path(), None, &requirements, &[]);
        let entry = &ledger.entries[0];
        assert!(entry.obligations.is_empty());
        assert_eq!(entry.earned_level, EvidenceLevel::Contracted);
        assert_eq!(
            entry.code.as_ref().map(|code| code.file.as_str()),
            Some("src/dose.c")
        );
    }

    #[test]
    fn requirement_without_outcomes_still_gets_an_entry() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("REQ-unmapped", None, None, false)];
        let ledger = build_ledger(dir.path(), None, &requirements, &[]);

        let entry = &ledger.entries[0];
        assert_eq!(entry.claimed_level, EvidenceLevel::None);
        assert_eq!(entry.earned_level, EvidenceLevel::None);
        assert!(entry.obligations.is_empty());
    }

    #[test]
    fn partial_merge_keeps_unaffected_entries_and_updates_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let full_requirements = vec![
            requirement("REQ-a", Some("src/a.c"), None, false),
            requirement("REQ-b", Some("src/b.c"), None, false),
        ];
        let full = build_ledger(
            dir.path(),
            None,
            &full_requirements,
            &[outcome("src/a.c", ObligationStatus::Proved)],
        );
        assert_eq!(full.entries[0].earned_level, EvidenceLevel::Proved);

        // A filtered re-run of REQ-b alone must not erase REQ-a's evidence.
        let partial_requirements = vec![requirement("REQ-b", Some("src/b.c"), None, false)];
        let partial = build_ledger(
            dir.path(),
            None,
            &partial_requirements,
            &[outcome("src/b.c", ObligationStatus::Proved)],
        );
        let merged = merge_partial(Some(full), partial);

        assert_eq!(merged.entries.len(), 2);
        assert_eq!(merged.entries[0].requirement_id, "REQ-a");
        assert_eq!(merged.entries[0].earned_level, EvidenceLevel::Proved);
        assert_eq!(merged.entries[1].requirement_id, "REQ-b");
        assert_eq!(merged.entries[1].earned_level, EvidenceLevel::Proved);

        // A requirement new since the last full run is appended.
        let new_req = vec![requirement("REQ-new", Some("src/n.c"), None, false)];
        let newer = build_ledger(dir.path(), None, &new_req, &[]);
        let merged = merge_partial(Some(merged), newer);
        assert_eq!(merged.entries.len(), 3);
        assert_eq!(merged.entries[2].requirement_id, "REQ-new");

        // No existing ledger: the partial stands alone.
        let solo = build_ledger(dir.path(), None, &new_req, &[]);
        assert_eq!(merge_partial(None, solo).entries.len(), 1);
    }

    #[test]
    fn seal_algorithm_is_pinned_for_cross_language_verifiers() {
        // The desktop re-derives this seal from the raw JSON (parse ->
        // compact re-stringify reproduces serde's compact form: key order
        // preserved, no numeric fields inside entries, same escape rules).
        // This hex is duplicated in the desktop's verify tests — change the
        // seal algorithm and both pins fail together.
        let entries = vec![LedgerEntry {
            requirement_id: "R".to_string(),
            code: None,
            claimed_level: EvidenceLevel::None,
            earned_level: EvidenceLevel::None,
            file_sha256: None,
            symbol_fingerprints: Vec::new(),
            requirement_sha256: String::new(),
            obligations: Vec::new(),
        }];
        assert_eq!(
            integrity_seal(1, None, &entries),
            "e050210e5bcabcc0479d2a2c1630871cb880fce4bc04d246818e49b9749a6a4d"
        );
    }

    #[test]
    fn a_passing_property_run_earns_property_tested_never_proved() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("PBT-1", Some("src/a.c"), None, false)];
        let passing = PropertyOutcome {
            requirement_id: "PBT-1".to_string(),
            file: "src/a.property.ts".to_string(),
            status: ObligationStatus::Proved,
            seed: 42,
            examples_run: 10_000,
        };

        let ledger = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[],
            std::slice::from_ref(&passing),
        );
        let entry = &ledger.entries[0];
        // Ten thousand passing examples are still statistical evidence.
        assert_eq!(entry.earned_level, EvidenceLevel::PropertyTested);
        assert_eq!(entry.obligations.len(), 1);
        assert_eq!(entry.obligations[0].kind, EvidenceKind::PropertyTest);

        // A failing run earns nothing above the mapping itself.
        let failing = PropertyOutcome {
            status: ObligationStatus::Failed,
            ..passing
        };
        let ledger = build_ledger_with_properties(dir.path(), None, &requirements, &[], &[failing]);
        assert_eq!(ledger.entries[0].earned_level, EvidenceLevel::SpecTracked);
    }

    #[test]
    fn a_partial_check_never_attaches_an_old_proof_to_a_new_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = "int f() { return 0; }";
        fs::write(root.join("a.c"), source).unwrap();
        let requirement = requirement("R", Some("a.c"), None, false);
        let proved = build_ledger(
            root,
            None,
            std::slice::from_ref(&requirement),
            &[outcome("a.c", ObligationStatus::Proved)],
        );
        let check_property_only = |req: &Requirement| {
            let requirements = std::slice::from_ref(req);
            let current = build_ledger_with_properties(
                root,
                None,
                requirements,
                &[],
                &[PropertyOutcome {
                    requirement_id: req.id.clone(),
                    file: "r.property.ts".to_string(),
                    status: ObligationStatus::Proved,
                    seed: 42,
                    examples_run: 100,
                }],
            );
            let merged = merge_run(
                Some(proved.clone()),
                current,
                requirements,
                false,
                true,
                false,
            );
            let entry = &merged.entries[0];
            assert_eq!(entry.earned_level, EvidenceLevel::PropertyTested);
            assert_eq!(entry.obligations.len(), 1);
            assert_eq!(entry.obligations[0].kind, EvidenceKind::PropertyTest);
            assert_eq!(summarize_earned(Some(&merged), 1).proved_requirements, 0);
        };

        let mut edited = requirement.clone();
        edited.requirement = "a different guarantee".to_string();
        check_property_only(&edited);

        let mut remapped = requirement.clone();
        fs::write(root.join("b.c"), source).unwrap();
        remapped.code.as_mut().unwrap().file = "b.c".to_string();
        check_property_only(&remapped);

        fs::write(root.join("a.c"), "int f() { return 1; }").unwrap();
        check_property_only(&requirement);
        fs::remove_file(root.join("a.c")).unwrap();
        check_property_only(&requirement);
    }

    #[test]
    fn a_property_only_run_keeps_the_proof_it_did_not_rerun() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.c"), "int f() { return 0; }").unwrap();
        let requirements = vec![requirement("BOTH-1", Some("src/a.c"), None, false)];

        // A full run proves the requirement.
        let full = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[outcome("src/a.c", ObligationStatus::Proved)],
            &[],
        );
        assert_eq!(full.entries[0].earned_level, EvidenceLevel::Proved);

        // The daemon then reruns ONLY property tests. Rebuilding from that
        // run alone would erase the proof and downgrade the requirement.
        let property_only = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[],
            &[PropertyOutcome {
                requirement_id: "BOTH-1".to_string(),
                file: "src/a.property.ts".to_string(),
                status: ObligationStatus::Proved,
                seed: 42,
                examples_run: 100,
            }],
        );
        assert_eq!(
            property_only.entries[0].earned_level,
            EvidenceLevel::PropertyTested,
            "the narrow run alone knows nothing of the proof"
        );

        let merged = merge_run(
            Some(full),
            property_only,
            &requirements,
            false, // proofs did not run
            true,  // properties did
            false, // scenarios did not run
        );
        let entry = &merged.entries[0];
        assert_eq!(
            entry.earned_level,
            EvidenceLevel::Proved,
            "evidence must survive a run that did not look for it"
        );
        assert_eq!(entry.obligations.len(), 2);
    }

    /// Regression for the ladder inversion: `PropertyTested` (E3) was matched
    /// before the `Contracted` (E4) fallback, so a contracted requirement that
    /// gained a passing property run DROPPED a rung and the ratchet read it as
    /// a regression.
    #[test]
    fn gaining_a_property_run_never_lowers_the_earned_rung() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("C-1", Some("src/a.c"), Some("Bounds"), false)];

        let contract_only = build_ledger(dir.path(), None, &requirements, &[]);
        assert_eq!(
            contract_only.entries[0].earned_level,
            EvidenceLevel::Contracted
        );

        let passing = PropertyOutcome {
            requirement_id: "C-1".to_string(),
            file: "src/a.property.ts".to_string(),
            status: ObligationStatus::Proved,
            seed: 42,
            examples_run: 10_000,
        };
        let with_property = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[],
            std::slice::from_ref(&passing),
        );

        assert!(
            with_property.entries[0].earned_level >= contract_only.entries[0].earned_level,
            "more evidence must never earn less: contract alone earned {:?}, \
             contract + passing property run earned {:?}",
            contract_only.entries[0].earned_level,
            with_property.entries[0].earned_level
        );
        // A contract still outranks statistical evidence; the ordering is
        // unchanged, only the resolution that ignored it.
        assert_eq!(
            with_property.entries[0].earned_level,
            EvidenceLevel::Contracted
        );
    }

    /// Regression for the unreachable rung: `Tested` (E2) was claimable but
    /// nothing could earn it, because the scenarios phase reported only
    /// failures and a passing run left no record.
    #[test]
    fn a_passing_scenario_run_earns_tested() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("S-1", Some("src/a.c"), None, false)];
        let passing = ScenarioOutcome {
            requirement_id: "S-1".to_string(),
            file: ".forall/scenarios/S-1.scenario.mjs".to_string(),
            status: ObligationStatus::Proved,
        };

        let ledger = build_ledger_with_evidence(
            dir.path(),
            None,
            &requirements,
            &[],
            &[],
            std::slice::from_ref(&passing),
        );
        let entry = &ledger.entries[0];
        assert_eq!(entry.earned_level, EvidenceLevel::Tested);
        assert_eq!(entry.obligations.len(), 1);
        assert_eq!(entry.obligations[0].kind, EvidenceKind::ScenarioTest);

        // A failing scenario earns nothing above the code mapping.
        let failing = ScenarioOutcome {
            status: ObligationStatus::Failed,
            ..passing
        };
        let failed = build_ledger_with_evidence(
            dir.path(),
            None,
            &requirements,
            &[],
            &[],
            std::slice::from_ref(&failing),
        );
        assert_eq!(failed.entries[0].earned_level, EvidenceLevel::SpecTracked);
    }

    /// Every rung that a mapping can CLAIM must be one machine evidence can
    /// EARN, or an assurance target set at that rung is unsatisfiable.
    #[test]
    fn every_claimable_rung_is_earnable() {
        let dir = tempfile::tempdir().unwrap();
        let earned = |req: Vec<Requirement>,
                      proofs: &[ProofOutcome],
                      props: &[PropertyOutcome],
                      scenarios: &[ScenarioOutcome]| {
            build_ledger_with_evidence(dir.path(), None, &req, proofs, props, scenarios).entries[0]
                .earned_level
        };
        let scenario = |id: &str| ScenarioOutcome {
            requirement_id: id.to_string(),
            file: "s.mjs".to_string(),
            status: ObligationStatus::Proved,
        };
        let property = |id: &str| PropertyOutcome {
            requirement_id: id.to_string(),
            file: "p.ts".to_string(),
            status: ObligationStatus::Proved,
            seed: 1,
            examples_run: 10,
        };

        assert_eq!(
            earned(vec![requirement("N", None, None, false)], &[], &[], &[]),
            EvidenceLevel::None
        );
        assert_eq!(
            earned(
                vec![requirement("T", Some("src/a.c"), None, false)],
                &[],
                &[],
                &[]
            ),
            EvidenceLevel::SpecTracked
        );
        assert_eq!(
            earned(
                vec![requirement("T", Some("src/a.c"), None, false)],
                &[],
                &[],
                &[scenario("T")]
            ),
            EvidenceLevel::Tested
        );
        assert_eq!(
            earned(
                vec![requirement("T", Some("src/a.c"), None, false)],
                &[],
                &[property("T")],
                &[]
            ),
            EvidenceLevel::PropertyTested
        );
        assert_eq!(
            earned(
                vec![requirement("T", Some("src/a.c"), Some("Bounds"), false)],
                &[],
                &[],
                &[]
            ),
            EvidenceLevel::Contracted
        );
        assert_eq!(
            earned(
                vec![requirement("T", Some("src/a.c"), None, false)],
                &[outcome("src/a.c", ObligationStatus::Proved)],
                &[],
                &[]
            ),
            EvidenceLevel::Proved
        );
    }

    #[test]
    fn a_run_that_skips_scenarios_keeps_the_scenario_evidence_it_had() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.c"), "int f() { return 0; }").unwrap();
        let requirements = vec![requirement("S-2", Some("src/a.c"), None, false)];
        let full = build_ledger_with_evidence(
            dir.path(),
            None,
            &requirements,
            &[],
            &[],
            &[ScenarioOutcome {
                requirement_id: "S-2".to_string(),
                file: "s.mjs".to_string(),
                status: ObligationStatus::Proved,
            }],
        );
        assert_eq!(full.entries[0].earned_level, EvidenceLevel::Tested);

        // A later run that does not look at scenarios must not erase them.
        let proofs_only = build_ledger(dir.path(), None, &requirements, &[]);
        let merged = merge_run(
            Some(full),
            proofs_only,
            &requirements,
            true,  // proofs ran
            false, // properties did not
            false, // scenarios did not
        );
        assert_eq!(
            merged.entries[0].earned_level,
            EvidenceLevel::Tested,
            "evidence must survive a run that did not look for it"
        );
    }

    #[test]
    fn a_rerun_replaces_its_own_phase_rather_than_accumulating() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("P-1", Some("src/a.c"), None, false)];
        let first = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[outcome("src/a.c", ObligationStatus::Proved)],
            &[],
        );
        // The same phase runs again and now fails.
        let second = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[outcome("src/a.c", ObligationStatus::Failed)],
            &[],
        );
        let merged = merge_run(Some(first), second, &requirements, true, false, false);
        assert_eq!(merged.entries[0].obligations.len(), 1, "no stale duplicate");
        assert_ne!(merged.entries[0].earned_level, EvidenceLevel::Proved);
    }

    #[test]
    fn a_proof_outranks_a_property_run_on_the_same_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("BOTH-1", Some("src/a.c"), None, false)];
        let ledger = build_ledger_with_properties(
            dir.path(),
            None,
            &requirements,
            &[outcome("src/a.c", ObligationStatus::Proved)],
            &[PropertyOutcome {
                requirement_id: "BOTH-1".to_string(),
                file: "src/a.property.ts".to_string(),
                status: ObligationStatus::Proved,
                seed: 42,
                examples_run: 100,
            }],
        );
        let entry = &ledger.entries[0];
        assert_eq!(entry.earned_level, EvidenceLevel::Proved);
        // Both kinds are recorded; the citation card can show each.
        assert_eq!(entry.obligations.len(), 2);
    }

    #[test]
    fn ledger_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("REQ-1", Some("src/a.c"), None, false)];
        let ledger = build_ledger(dir.path(), Some("add-clamp"), &requirements, &[]);

        write_ledger(dir.path(), &ledger).unwrap();
        let read = read_ledger(dir.path(), Some("add-clamp")).expect("ledger reads back");
        assert_eq!(read.version, LEDGER_VERSION);
        assert_eq!(read.entries.len(), 1);
        assert_eq!(read.entries[0].requirement_id, "REQ-1");
    }

    #[test]
    fn tampered_ledger_reads_as_no_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("REQ-1", Some("src/a.c"), None, false)];
        let ledger = build_ledger(dir.path(), None, &requirements, &[]);
        write_ledger(dir.path(), &ledger).unwrap();
        let path = ledger_json_path(dir.path(), None);

        // A hand edit that forges an earned level breaks the seal: the
        // ledger fails CLOSED to "no evidence", never to forged evidence.
        let forged = fs::read_to_string(&path)
            .unwrap()
            .replace("\"spec_tracked\"", "\"proved\"");
        fs::write(&path, forged).unwrap();
        assert!(read_ledger(dir.path(), None).is_none());

        // A missing or legacy (pre-seal) integrity field also fails closed.
        let unsealed = serde_json::to_string(&Ledger {
            integrity: String::new(),
            ..build_ledger(dir.path(), None, &requirements, &[])
        })
        .unwrap();
        fs::write(&path, unsealed).unwrap();
        assert!(read_ledger(dir.path(), None).is_none());
    }

    #[test]
    fn earned_summary_totals_come_from_the_ledger_itself() {
        let dir = tempfile::tempdir().unwrap();
        let full_requirements = vec![
            requirement("REQ-a", Some("src/a.c"), None, false),
            requirement("REQ-b", Some("src/b.c"), None, false),
        ];
        let full = build_ledger(
            dir.path(),
            None,
            &full_requirements,
            &[
                outcome("src/a.c", ObligationStatus::Proved),
                outcome("src/b.c", ObligationStatus::Proved),
            ],
        );

        // A filtered run passes its 1-requirement slice as fallback, but the
        // summary must count against the merged ledger's 2 entries — never
        // "2 proved of 1 total".
        let summary = summarize_earned(Some(&full), 1);
        assert_eq!(summary.total_requirements, 2);
        assert_eq!(summary.proved_requirements, 2);
        assert!(summary.proved_requirements <= summary.total_requirements);

        // No ledger yet: nothing has been machine-checked.
        let empty = summarize_earned(None, 3);
        assert_eq!(empty.total_requirements, 3);
        assert_eq!(empty.proved_requirements, 0);
        assert_eq!(empty.spec_tracked_requirements, 3);
    }
}
