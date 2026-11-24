//! Graded staleness: what changed since the evidence was earned, and
//! therefore what the reader should do about it.
//!
//! The grades mean different things and must never collapse into one
//! "stale" flag: code drift says the proof may not hold; spec drift says
//! the proof holds but of the old claim; toolchain drift says only a replay
//! is owed. Dependency drift (a callee's contract changed) arrives with
//! call-graph extraction. `Unknown` is the honest grade when a cited symbol
//! can no longer be resolved — an unanchored citation must never render as
//! fresh.

use std::fs;
use std::path::Path;

use crate::mapping::schema::Requirement;
use crate::verify::fingerprint::{FingerprintLanguage, symbol_fingerprints};
use crate::verify::ledger::{LedgerEntry, requirement_text_sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StalenessGrade {
    /// Evidence still speaks for the current code, spec, and toolchain.
    Fresh,
    /// A cited symbol's code changed — the proof may not hold anymore.
    CodeDrift,
    /// The formal annotations or the requirement sentence changed — the
    /// proof holds, but of the old claim.
    SpecDrift,
    /// The prover version moved — the verdict deserves a replay, nothing
    /// more.
    ToolchainDrift,
    /// A cited symbol cannot be resolved against the current source; the
    /// evidence cannot be graded and must not render as fresh.
    Unknown,
}

/// Grade a ledger entry against the current workspace. `current_tool_version`
/// resolves a tool id to its present version (None when unavailable) so the
/// caller decides whether probing is affordable.
pub fn grade_entry(
    root: &Path,
    entry: &LedgerEntry,
    requirement: &Requirement,
    current_tool_version: impl Fn(&str) -> Option<String>,
) -> StalenessGrade {
    let mut spec_drift = !entry.requirement_sha256.is_empty()
        && entry.requirement_sha256 != requirement_text_sha256(requirement);

    let mut code_drift = false;
    let mut unknown = false;

    match &requirement.code {
        None => {
            // The evidence was code-anchored but the requirement no longer
            // cites any code: unanchored evidence cannot be called fresh.
            // The recorded citation counts even when the cited file was
            // unreadable at check time and left no fingerprints or hash.
            if entry.code.is_some()
                || !entry.symbol_fingerprints.is_empty()
                || entry.file_sha256.is_some()
            {
                unknown = true;
            }
        }
        Some(code) => {
            // Evidence is anchored to the file it was earned against: a
            // requirement remapped to a different file must not inherit
            // freshness from a same-named, structurally identical symbol
            // there. Legacy entries that carry fingerprints but no recorded
            // citation are equally ungradeable.
            let citation_moved = match &entry.code {
                Some(recorded) => recorded.file != code.file,
                None => !entry.symbol_fingerprints.is_empty(),
            };
            if citation_moved {
                unknown = true;
            }
            let language = FingerprintLanguage::for_path(&code.file);
            let source = fs::read_to_string(root.join(&code.file)).ok();
            // Grade against the CURRENT citation set, not the stored one: a
            // symbol the baseline never resolved, or one newly added to the
            // mapping, is an unchecked citation — Unknown, even when every
            // stored symbol is untouched. Baseline records for symbols no
            // longer cited are ignored; they stopped being citations.
            for symbol in code.symbols.iter().filter(|_| !citation_moved) {
                let record = entry
                    .symbol_fingerprints
                    .iter()
                    .find(|record| record.symbol == *symbol);
                let Some(record) = record else {
                    unknown = true;
                    continue;
                };
                let current = match (language, source.as_deref()) {
                    (Some(language), Some(source)) => symbol_fingerprints(source, language, symbol),
                    _ => None,
                };
                match current {
                    None => unknown = true,
                    Some(current) => {
                        if current.code != record.code {
                            code_drift = true;
                        } else if current.full != record.full {
                            spec_drift = true;
                        }
                    }
                }
            }
        }
    }

    let toolchain_drift = entry.obligations.iter().any(|obligation| {
        obligation.tool_version.as_ref().is_some_and(|recorded| {
            current_tool_version(&obligation.tool).as_ref() != Some(recorded)
        })
    });

    if code_drift {
        StalenessGrade::CodeDrift
    } else if spec_drift {
        StalenessGrade::SpecDrift
    } else if unknown {
        StalenessGrade::Unknown
    } else if toolchain_drift {
        StalenessGrade::ToolchainDrift
    } else {
        StalenessGrade::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::CodeRef;
    use crate::verify::ledger::{ObligationStatus, ProofOutcome, build_ledger};

    fn requirement(file: &str, symbol: &str) -> Requirement {
        Requirement {
            id: "REQ-1".to_string(),
            capability: "cap".to_string(),
            requirement: "The parser must reject bad frames.".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: file.to_string(),
                symbols: vec![symbol.to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        }
    }

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    const FRESH_TOOL: fn(&str) -> Option<String> = |_| Some("Frama-C 32.1".to_string());

    fn baseline(root: &Path) -> (LedgerEntry, Requirement) {
        write(
            root,
            "src/frame.c",
            "//@ requires len > 0;\nint parse(int len) { return len; }\n",
        );
        let req = requirement("src/frame.c", "parse");
        let ledger = build_ledger(
            root,
            None,
            std::slice::from_ref(&req),
            &[ProofOutcome {
                file: "src/frame.c".to_string(),
                tool: "framac".to_string(),
                tool_version: Some("Frama-C 32.1".to_string()),
                status: ObligationStatus::Proved,
            }],
        );
        (ledger.entries.into_iter().next().unwrap(), req)
    }

    #[test]
    fn untouched_workspace_grades_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, req) = baseline(dir.path());
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Fresh
        );
        // Reformatting is not drift of any kind.
        write(
            dir.path(),
            "src/frame.c",
            "//@   requires len > 0;\nint parse(int len)\n{\n    return len;\n}\n",
        );
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Fresh
        );
    }

    #[test]
    fn body_edit_is_code_drift_and_wins_over_other_grades() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, req) = baseline(dir.path());
        write(
            dir.path(),
            "src/frame.c",
            "//@ requires len >= 0;\nint parse(int len) { return len + 1; }\n",
        );
        // Body AND annotation changed, tool moved too: code drift wins.
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, |_| Some("Frama-C 33.0".into())),
            StalenessGrade::CodeDrift
        );
    }

    #[test]
    fn annotation_or_requirement_edit_is_spec_drift() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, req) = baseline(dir.path());
        write(
            dir.path(),
            "src/frame.c",
            "//@ requires len >= 0;\nint parse(int len) { return len; }\n",
        );
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::SpecDrift
        );

        // Same code, changed requirement sentence: still spec drift.
        let (entry, mut req) = baseline(dir.path());
        req.requirement = "The parser must accept bad frames.".to_string();
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::SpecDrift
        );
    }

    #[test]
    fn tool_version_change_alone_is_toolchain_drift() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, req) = baseline(dir.path());
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, |_| Some("Frama-C 33.0".into())),
            StalenessGrade::ToolchainDrift
        );
    }

    #[test]
    fn uncovered_citations_grade_unknown_even_when_stored_symbols_are_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, mut req) = baseline(dir.path());

        // A symbol added to the mapping after the baseline was recorded is
        // an unchecked citation.
        req.code
            .as_mut()
            .unwrap()
            .symbols
            .push("added_later".to_string());
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Unknown
        );

        // The same applies when the baseline only partially resolved: keep
        // the recorded symbol untouched, cite one the ledger never covered.
        let (entry, mut req) = baseline(dir.path());
        req.code.as_mut().unwrap().symbols =
            vec!["parse".to_string(), "never_resolved".to_string()];
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Unknown
        );

        // A stored record for a symbol no longer cited is not a citation
        // anymore and must not block Fresh.
        let (entry, mut req) = baseline(dir.path());
        req.code.as_mut().unwrap().symbols = vec!["parse".to_string()];
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Fresh
        );
    }

    #[test]
    fn hashless_anchored_entry_with_dropped_mapping_is_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        // Baseline recorded while the cited file was unreadable: the entry
        // carries its citation but no fingerprints and no file hash.
        let req = requirement("src/missing.c", "parse");
        let ledger = build_ledger(dir.path(), None, std::slice::from_ref(&req), &[]);
        let entry = ledger.entries.into_iter().next().unwrap();
        assert!(entry.symbol_fingerprints.is_empty());
        assert_eq!(entry.file_sha256, None);
        assert!(entry.code.is_some());

        let mut unmapped = req;
        unmapped.code = None;
        assert_eq!(
            grade_entry(dir.path(), &entry, &unmapped, FRESH_TOOL),
            StalenessGrade::Unknown
        );
    }

    #[test]
    fn remapping_to_an_identical_symbol_elsewhere_is_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, mut req) = baseline(dir.path());
        // A byte-identical twin in another file: same name, same structure.
        let twin = fs::read_to_string(dir.path().join("src/frame.c")).unwrap();
        write(dir.path(), "src/other.c", &twin);
        req.code.as_mut().unwrap().file = "src/other.c".to_string();

        // The evidence was earned against src/frame.c; the citation moved.
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Unknown
        );
    }

    #[test]
    fn removed_code_mapping_grades_unknown_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, mut req) = baseline(dir.path());
        req.code = None;
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Unknown
        );
    }

    #[test]
    fn unresolvable_symbol_is_unknown_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (entry, req) = baseline(dir.path());
        write(
            dir.path(),
            "src/frame.c",
            "int renamed(int len) { return len; }\n",
        );
        assert_eq!(
            grade_entry(dir.path(), &entry, &req, FRESH_TOOL),
            StalenessGrade::Unknown
        );
    }
}
