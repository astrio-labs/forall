//! Per-path assurance targets and the ratchet — the economic layer of the
//! evidence ladder.
//!
//! `.forall/assurance.yaml` declares what each part of the tree owes:
//! a minimum evidence rung per glob, or an explicit exemption. Exemption is
//! a first-class positive state — "this code legitimately needs no
//! citations" — so the overlay reads as a progress map, not a shame index.
//! The committed baseline (`.forall/assurance-baseline.yaml`) is the
//! ratchet: `forall assurance check` fails only on REGRESSION below the
//! recorded high-water mark or on targeted paths falling short, never on
//! absolute levels a team inherited — that distinction is what makes
//! quality gates survive contact with real repositories. Waivers carry an
//! expiry: an escape hatch that cannot quietly become policy.
//!
//! Levels come from the obligation ledger's EARNED rungs, never from
//! mapping claims.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::mapping::schema::EvidenceLevel;
use crate::verify::ledger::{Ledger, LedgerEntry};

pub const ASSURANCE_PATH: &str = ".forall/assurance.yaml";
pub const BASELINE_PATH: &str = ".forall/assurance-baseline.yaml";
pub const WAIVERS_PATH: &str = ".forall/assurance-waivers.yaml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", untagged)]
pub enum TargetLevel {
    /// `exempt` — declared out of assurance scope; renders neutral.
    Marker(ExemptMarker),
    /// A minimum rung on the ladder.
    Min(EvidenceLevel),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExemptMarker {
    Exempt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetRule {
    /// Glob over project-relative file paths (the requirement's cited file).
    pub path: String,
    pub level: TargetLevel,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssuranceConfig {
    #[serde(default)]
    pub targets: Vec<TargetRule>,
}

/// The committed high-water marks, one line per requirement, sorted — the
/// format is deliberately merge-friendly (per-requirement granularity;
/// per-line records) because conflict-prone ratchet files are how coverage
/// ratchets die in monorepos.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Baseline {
    #[serde(default)]
    pub requirements: BTreeMap<String, EvidenceLevel>,
}

pub fn load_config(root: &Path) -> Result<AssuranceConfig, String> {
    let path = root.join(ASSURANCE_PATH);
    if !path.is_file() {
        return Ok(AssuranceConfig::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_yaml::from_str(&raw).map_err(|e| format!("{ASSURANCE_PATH}: {e}"))
}

pub fn load_baseline(root: &Path) -> Result<Baseline, String> {
    let path = root.join(BASELINE_PATH);
    if !path.is_file() {
        return Ok(Baseline::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_yaml::from_str(&raw).map_err(|e| format!("{BASELINE_PATH}: {e}"))
}

pub fn save_baseline(root: &Path, baseline: &Baseline) -> Result<(), String> {
    let path = root.join(BASELINE_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        &path,
        serde_yaml::to_string(baseline).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Match a project-relative path against a rule glob. Deliberately small:
/// `**` crosses directory separators, `*` does not, no character classes —
/// documented behavior over glob-crate surprises.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    fn inner(pattern: &[char], path: &[char]) -> bool {
        match (pattern.first(), path.first()) {
            (None, None) => true,
            (Some('*'), _) if pattern.get(1) == Some(&'*') => {
                // `**` may consume anything, including nothing and `/`.
                let rest = if pattern.get(2) == Some(&'/') {
                    &pattern[3..]
                } else {
                    &pattern[2..]
                };
                (0..=path.len()).any(|skip| inner(rest, &path[skip..]))
            }
            (Some('*'), _) => {
                // `*` consumes anything but `/`.
                (0..=path.len())
                    .take_while(|&skip| skip == 0 || path[skip - 1] != '/')
                    .any(|skip| inner(&pattern[1..], &path[skip..]))
            }
            (Some(p), Some(c)) if p == c => inner(&pattern[1..], &path[1..]),
            _ => false,
        }
    }
    inner(
        &pattern.chars().collect::<Vec<_>>(),
        &path.chars().collect::<Vec<_>>(),
    )
}

/// The target owed by a cited file: the LAST matching rule wins, so teams
/// can declare a broad default and carve out specifics below it.
pub fn target_for_path<'a>(config: &'a AssuranceConfig, path: &str) -> Option<&'a TargetLevel> {
    config
        .targets
        .iter()
        .rev()
        .find(|rule| glob_matches(&rule.path, path))
        .map(|rule| &rule.level)
}

/// A time-boxed exception. Expiry is the design: an escape hatch that
/// cannot quietly become policy — expired waivers stop waiving and are
/// reported, so the gate re-tightens on its own.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Waiver {
    pub requirement_id: String,
    /// ISO date (YYYY-MM-DD); the waiver stops applying the day after.
    pub expires: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Waivers {
    #[serde(default)]
    pub waivers: Vec<Waiver>,
}

pub fn load_waivers(root: &Path) -> Result<Waivers, String> {
    let path = root.join(WAIVERS_PATH);
    if !path.is_file() {
        return Ok(Waivers::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    serde_yaml::from_str(&raw).map_err(|e| format!("{WAIVERS_PATH}: {e}"))
}

pub fn save_waivers(root: &Path, waivers: &Waivers) -> Result<(), String> {
    let path = root.join(WAIVERS_PATH);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        &path,
        serde_yaml::to_string(waivers).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

impl Waivers {
    /// Whether `requirement_id` holds an unexpired waiver on `today`
    /// (ISO date). String comparison is date comparison for ISO dates.
    pub fn covers(&self, requirement_id: &str, today: &str) -> bool {
        self.waivers.iter().any(|waiver| {
            waiver.requirement_id == requirement_id && waiver.expires.as_str() >= today
        })
    }

    pub fn expired(&self, today: &str) -> impl Iterator<Item = &Waiver> {
        self.waivers
            .iter()
            .filter(move |waiver| waiver.expires.as_str() < today)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A requirement fell below the committed baseline: the only absolute
    /// failure — progress is never given back.
    Regression {
        requirement_id: String,
        baseline: EvidenceLevel,
        earned: EvidenceLevel,
    },
    /// A targeted path is below its declared minimum.
    BelowTarget {
        requirement_id: String,
        file: String,
        target: EvidenceLevel,
        earned: EvidenceLevel,
    },
}

#[derive(Debug, Clone, Default)]
pub struct AssuranceReport {
    pub findings: Vec<Finding>,
    /// Findings suppressed by an unexpired waiver, kept visible: a waiver
    /// hides nothing, it only defers failure.
    pub waived: Vec<Finding>,
    /// Requirements whose earned level rose above the recorded baseline —
    /// written back so improvement locks in automatically.
    pub ratcheted: Vec<String>,
    pub exempt: usize,
    pub untargeted: usize,
}

/// Evaluate the ledger against targets and the committed baseline, and
/// raise the baseline where evidence improved. Pure over its inputs; the
/// caller decides whether to persist the ratcheted baseline.
pub fn evaluate(
    config: &AssuranceConfig,
    baseline: &mut Baseline,
    ledger: &Ledger,
) -> AssuranceReport {
    let mut report = AssuranceReport::default();
    for entry in &ledger.entries {
        evaluate_entry(config, baseline, entry, &mut report);
    }
    report
}

/// `evaluate`, then move findings covered by an unexpired waiver into
/// `waived`. The baseline ratchet is untouched by waivers: a waiver defers
/// the gate, never the record.
pub fn evaluate_with_waivers(
    config: &AssuranceConfig,
    baseline: &mut Baseline,
    ledger: &Ledger,
    waivers: &Waivers,
    today: &str,
) -> AssuranceReport {
    let mut report = evaluate(config, baseline, ledger);
    let (waived, live): (Vec<_>, Vec<_>) = report.findings.drain(..).partition(|finding| {
        let id = match finding {
            Finding::Regression { requirement_id, .. }
            | Finding::BelowTarget { requirement_id, .. } => requirement_id,
        };
        waivers.covers(id, today)
    });
    report.findings = live;
    report.waived = waived;
    report
}

fn evaluate_entry(
    config: &AssuranceConfig,
    baseline: &mut Baseline,
    entry: &LedgerEntry,
    report: &mut AssuranceReport,
) {
    let earned = entry.earned_level;

    // Ratchet against the committed high-water mark, independent of targets.
    match baseline.requirements.get(&entry.requirement_id) {
        Some(&recorded) if earned < recorded => {
            report.findings.push(Finding::Regression {
                requirement_id: entry.requirement_id.clone(),
                baseline: recorded,
                earned,
            });
        }
        Some(&recorded) if earned > recorded => {
            baseline
                .requirements
                .insert(entry.requirement_id.clone(), earned);
            report.ratcheted.push(entry.requirement_id.clone());
        }
        Some(_) => {}
        None => {
            baseline
                .requirements
                .insert(entry.requirement_id.clone(), earned);
            report.ratcheted.push(entry.requirement_id.clone());
        }
    }

    // Delta to target, when the cited file is under a declared rule.
    let Some(file) = entry.code.as_ref().map(|code| code.file.as_str()) else {
        report.untargeted += 1;
        return;
    };
    match target_for_path(config, file) {
        None => report.untargeted += 1,
        Some(TargetLevel::Marker(ExemptMarker::Exempt)) => report.exempt += 1,
        Some(TargetLevel::Min(minimum)) => {
            if earned < *minimum {
                report.findings.push(Finding::BelowTarget {
                    requirement_id: entry.requirement_id.clone(),
                    file: file.to_string(),
                    target: *minimum,
                    earned,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::{CodeRef, Requirement};
    use crate::verify::ledger::build_ledger;

    fn requirement(id: &str, file: &str, verified: bool) -> Requirement {
        Requirement {
            id: id.to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: file.to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        }
    }

    fn config(yaml: &str) -> AssuranceConfig {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn globs_match_documented_semantics() {
        assert!(glob_matches("src/payments/**", "src/payments/core/pay.c"));
        assert!(glob_matches("src/payments/**", "src/payments/pay.c"));
        assert!(!glob_matches("src/payments/*", "src/payments/core/pay.c"));
        assert!(glob_matches("src/payments/*", "src/payments/pay.c"));
        assert!(glob_matches("**/*.c", "deep/nested/file.c"));
        assert!(!glob_matches("src/*.c", "src/nested/file.c"));
    }

    #[test]
    fn last_matching_rule_wins_and_exempt_is_first_class() {
        let config = config(
            "targets:\n  - path: 'src/**'\n    level: property_tested\n  - path: 'src/ui/**'\n    level: exempt\n",
        );
        assert_eq!(
            target_for_path(&config, "src/pay.c"),
            Some(&TargetLevel::Min(EvidenceLevel::PropertyTested))
        );
        assert_eq!(
            target_for_path(&config, "src/ui/panel.ts"),
            Some(&TargetLevel::Marker(ExemptMarker::Exempt))
        );
        assert_eq!(target_for_path(&config, "docs/readme.md"), None);
    }

    #[test]
    fn ratchet_locks_in_progress_and_fails_only_on_regression() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/pay.c"), "int f(void){return 0;}\n").unwrap();

        let requirements = vec![requirement("PAY-1", "src/pay.c", true)];
        let proved = build_ledger(
            dir.path(),
            None,
            &requirements,
            &[crate::verify::ledger::ProofOutcome {
                file: "src/pay.c".to_string(),
                tool: "framac".to_string(),
                tool_version: None,
                status: crate::verify::ledger::ObligationStatus::Proved,
            }],
        );
        let config = AssuranceConfig::default();
        let mut baseline = Baseline::default();

        // First run records the high-water mark.
        let first = evaluate(&config, &mut baseline, &proved);
        assert!(first.findings.is_empty());
        assert_eq!(first.ratcheted, vec!["PAY-1".to_string()]);
        assert_eq!(
            baseline.requirements.get("PAY-1"),
            Some(&EvidenceLevel::Proved)
        );

        // A later run that earns less is a regression, and the baseline
        // does NOT drop to meet it.
        let failed = build_ledger(dir.path(), None, &requirements, &[]);
        let second = evaluate(&config, &mut baseline, &failed);
        assert_eq!(second.findings.len(), 1);
        assert!(matches!(
            &second.findings[0],
            Finding::Regression { requirement_id, baseline, .. }
                if requirement_id == "PAY-1" && *baseline == EvidenceLevel::Proved
        ));
        assert_eq!(
            baseline.requirements.get("PAY-1"),
            Some(&EvidenceLevel::Proved)
        );
    }

    #[test]
    fn waivers_defer_findings_until_expiry_and_never_touch_the_ratchet() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![requirement("PAY-1", "src/payments/pay.c", false)];
        let ledger = build_ledger(dir.path(), None, &requirements, &[]);
        let config = config("targets:\n  - path: 'src/payments/**'\n    level: proved\n");
        let waivers: Waivers = serde_yaml::from_str(
            "waivers:\n  - requirement_id: PAY-1\n    expires: '2099-12-31'\n    reason: prover migration\n",
        )
        .unwrap();

        // Unexpired: the finding moves to `waived`, the gate sees none.
        let mut baseline = Baseline::default();
        let report = evaluate_with_waivers(&config, &mut baseline, &ledger, &waivers, "2026-08-03");
        assert!(report.findings.is_empty());
        assert_eq!(report.waived.len(), 1);
        // The ratchet recorded the honest level regardless of the waiver.
        assert_eq!(
            baseline.requirements.get("PAY-1"),
            Some(&EvidenceLevel::SpecTracked)
        );

        // Expired: the same waiver waives nothing.
        let expired: Waivers = serde_yaml::from_str(
            "waivers:\n  - requirement_id: PAY-1\n    expires: '2026-01-01'\n    reason: old\n",
        )
        .unwrap();
        let mut baseline = Baseline::default();
        let report = evaluate_with_waivers(&config, &mut baseline, &ledger, &expired, "2026-08-03");
        assert_eq!(report.findings.len(), 1);
        assert!(report.waived.is_empty());
        assert_eq!(expired.expired("2026-08-03").count(), 1);
    }

    #[test]
    fn targets_report_delta_and_exemption_counts() {
        let dir = tempfile::tempdir().unwrap();
        let requirements = vec![
            requirement("PAY-1", "src/payments/pay.c", false),
            requirement("UI-1", "src/ui/panel.ts", false),
            requirement("DOC-1", "docs/notes.md", false),
        ];
        let ledger = build_ledger(dir.path(), None, &requirements, &[]);
        let config = config(
            "targets:\n  - path: 'src/payments/**'\n    level: proved\n  - path: 'src/ui/**'\n    level: exempt\n",
        );
        let mut baseline = Baseline::default();
        let report = evaluate(&config, &mut baseline, &ledger);

        assert_eq!(report.exempt, 1);
        assert_eq!(report.untargeted, 1);
        assert_eq!(report.findings.len(), 1);
        assert!(matches!(
            &report.findings[0],
            Finding::BelowTarget { requirement_id, target, .. }
                if requirement_id == "PAY-1" && *target == EvidenceLevel::Proved
        ));
    }
}
