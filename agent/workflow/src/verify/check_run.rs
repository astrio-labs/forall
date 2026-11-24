use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use crate::mapping::load::{mapping_version_for_check, requirements_for_check};
use crate::mapping::schema::{IssueSeverity, PhaseStatus, Requirement, VerifyIssue, VerifyReport};
use crate::mapping::validate::{validate_mapping_structure, validate_symbols_against_extract};
use crate::paths::workflow_config_read_path;
use crate::verify::adapters::{
    PROOF_FRAMAC, PROOF_LEMMASCRIPT_DAFNY, PROOF_OPENJML, PROOF_VERUS, load_verified_config,
    proof_eligible_files, validate_adapter_requirements,
};
use crate::verify::backends::dafny_install::{
    dafny_install_hint, dafny_is_available, ensure_dafny_toolchain,
};
use crate::verify::backends::framac_install::{
    ensure_framac_toolchain, framac_install_hint, framac_is_available,
};
use crate::verify::backends::lsc_install::{
    ensure_lsc_toolchain, lsc_install_hint, lsc_is_available,
};
use crate::verify::backends::openjml_install::{
    ensure_openjml_toolchain, openjml_install_hint, openjml_is_available,
};
use crate::verify::backends::verus_install::{
    ensure_verus_toolchain, verus_install_hint, verus_is_available,
};
use crate::verify::backends::{
    BackendCheckOutput, ProofBackend, backend_for_proof_id, collect_extracts,
    group_files_by_proof_backend,
};
use crate::verify::dfy_merge::validate_dfy_merge_artifacts;
use crate::verify::engine::lsc_error_detail;
use crate::verify::intent::{intent_phase_status, validate_intent, write_claimcheck_records};
use crate::verify::ledger::{
    ObligationStatus, ProofOutcome, build_ledger_with_evidence, merge_partial, merge_run,
    read_ledger, summarize_earned, write_ledger,
};
use crate::verify::properties::validate_change_properties;
use crate::verify::property_tests::{
    PropertyTestOptions, property_tests_phase_status, run_property_tests, validate_property_files,
};
use crate::verify::report::default_phases;
use crate::verify::scenario_tests::{run_scenario_tests, scenario_tests_phase_status};
use crate::verify::scenarios::{scenarios_phase_status, validate_scenarios};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CheckPhase {
    Structure,
    Mapping,
    Proofs,
    Intent,
    Scenarios,
    PropertyTests,
    ScenarioTests,
}

impl CheckPhase {
    pub fn all() -> BTreeSet<Self> {
        BTreeSet::from([
            Self::Structure,
            Self::Mapping,
            Self::Proofs,
            Self::Intent,
            Self::Scenarios,
            Self::PropertyTests,
            Self::ScenarioTests,
        ])
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Mapping => "mapping",
            Self::Proofs => "proofs",
            Self::Intent => "intent",
            Self::Scenarios => "scenarios",
            Self::PropertyTests => "property-tests",
            Self::ScenarioTests => "scenario-tests",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PhasePlan {
    pub phases: BTreeSet<CheckPhase>,
    pub requirement_ids: Option<HashSet<String>>,
}

impl PhasePlan {
    pub fn full() -> Self {
        Self {
            phases: CheckPhase::all(),
            requirement_ids: None,
        }
    }

    pub fn runs(&self, phase: CheckPhase) -> bool {
        self.phases.contains(&phase)
    }
}

pub struct CheckExecutionOptions<'a> {
    pub root: &'a Path,
    pub change: Option<&'a str>,
    pub strict: bool,
    pub pbt_seed: u64,
    pub pbt_examples: u32,
    pub plan: PhasePlan,
    pub control: CheckExecutionControl<'a>,
}

pub struct CheckExecutionControl<'a> {
    pub install_missing_toolchains: bool,
    pub on_phase: Option<&'a dyn Fn(CheckPhase)>,
    pub is_cancelled: Option<&'a dyn Fn() -> bool>,
}

impl CheckExecutionControl<'_> {
    pub fn local() -> Self {
        Self {
            install_missing_toolchains: true,
            on_phase: None,
            is_cancelled: None,
        }
    }

    pub fn hosted<'a>(
        on_phase: Option<&'a dyn Fn(CheckPhase)>,
        is_cancelled: Option<&'a dyn Fn() -> bool>,
    ) -> CheckExecutionControl<'a> {
        CheckExecutionControl {
            install_missing_toolchains: false,
            on_phase,
            is_cancelled,
        }
    }

    fn enter(&self, phase: CheckPhase) -> Result<(), String> {
        if self.is_cancelled.is_some_and(|is_cancelled| is_cancelled()) {
            return Err("verification cancelled".to_string());
        }
        if let Some(on_phase) = self.on_phase {
            on_phase(phase);
        }
        Ok(())
    }
}

fn filter_requirements(
    requirements: Vec<Requirement>,
    ids: &Option<HashSet<String>>,
) -> Vec<Requirement> {
    match ids {
        Some(set) if !set.is_empty() => requirements
            .into_iter()
            .filter(|r| set.contains(&r.id))
            .collect(),
        _ => requirements,
    }
}

fn mark_skipped_phases(phases: &mut BTreeMap<String, PhaseStatus>, plan: &PhasePlan) {
    let skip = |phases: &mut BTreeMap<String, PhaseStatus>, name: &str| {
        if !phases.contains_key(name) {
            phases.insert(name.to_string(), PhaseStatus::Skip);
        }
    };
    if !plan.runs(CheckPhase::Structure) {
        skip(phases, "structure");
    }
    if !plan.runs(CheckPhase::Mapping) {
        skip(phases, "mapping");
    }
    if !plan.runs(CheckPhase::Proofs) {
        skip(phases, "proofs");
    }
    if !plan.runs(CheckPhase::Intent) {
        skip(phases, "intent");
    }
    if !plan.runs(CheckPhase::Scenarios) {
        skip(phases, "scenarios");
    }
    if !plan.runs(CheckPhase::PropertyTests) {
        skip(phases, "property-tests");
    }
    if !plan.runs(CheckPhase::ScenarioTests) {
        skip(phases, "scenario-tests");
    }
}

/// The dispatched files a backend output's label speaks for: the label
/// itself for per-file backends, or the files under a directory label for
/// whole-crate backends like Verus (empty label = repo root). `files` is the
/// group actually handed to this backend, which keeps a crate label from
/// claiming files another backend owns.
fn files_covered_by_output<'a>(label: &str, files: &'a [String]) -> Vec<&'a str> {
    if let Some(exact) = files.iter().find(|file| *file == label) {
        return vec![exact.as_str()];
    }
    let prefix = label.trim_end_matches('/');
    files
        .iter()
        .filter(|file| {
            prefix.is_empty()
                || file.strip_prefix(prefix).is_some_and(|rest| {
                    // "src/a.rs" is under "src", but "srcfoo/a.rs" is not.
                    rest.starts_with('/')
                })
        })
        .map(String::as_str)
        .collect()
}

/// Requirement ids a backend output speaks for, so a proof verdict can name
/// the requirements it affects instead of only a tool label.
fn requirement_ids_for_output<'a>(
    requirements: &'a [Requirement],
    label: &str,
    files: &[String],
) -> Vec<&'a str> {
    let covered = files_covered_by_output(label, files);
    requirements
        .iter()
        .filter(|req| {
            req.code
                .as_ref()
                .is_some_and(|code| covered.contains(&code.file.as_str()))
        })
        .map(|req| req.id.as_str())
        .collect()
}

fn proof_failure_issues(
    backend: &dyn ProofBackend,
    output: &BackendCheckOutput,
    toolchain_available: bool,
    requirement_ids: &[&str],
) -> [VerifyIssue; 2] {
    let detail = backend.error_detail(&output.result);
    // "tool:" (not "command:") so the hosted report sanitizer keeps the line.
    let tool_line = match &output.result.tool_version {
        Some(version) => format!("tool: {} {version}\n", backend.id()),
        None => format!("tool: {}\n", backend.id()),
    };
    let proof_detail = format!(
        "{tool_line}command: {}\nstdout:\n{}\nstderr:\n{}",
        output.result.command, output.result.stdout, output.result.stderr
    );
    // VerifyIssue carries one requirement id, so only an unambiguous mapping
    // is attributed there; multiple matches still reach the reader via the
    // message.
    let sole_requirement = match requirement_ids {
        [only] => Some((*only).to_string()),
        _ => None,
    };
    let message = if requirement_ids.is_empty() {
        format!("proof check failed for {}: {detail}", output.label)
    } else {
        format!(
            "proof check failed for {} (requirements: {}): {detail}",
            output.label,
            requirement_ids.join(", ")
        )
    };
    [
        VerifyIssue {
            severity: IssueSeverity::Critical,
            phase: "proofs".to_string(),
            requirement_id: sole_requirement.clone(),
            message,
            file: Some(output.label.clone()),
            counterexample: None,
            pbt: None,
            proof_detail: Some(proof_detail),
        },
        VerifyIssue {
            severity: IssueSeverity::Info,
            phase: "proofs".to_string(),
            requirement_id: sole_requirement,
            message: if toolchain_available {
                backend.proof_failure_hint(&output.result)
            } else {
                backend.install_hint()
            },
            file: Some(output.label.clone()),
            counterexample: None,
            pbt: None,
            proof_detail: None,
        },
    ]
}

pub fn execute_check(opts: CheckExecutionOptions<'_>) -> Result<VerifyReport, String> {
    let all_requirements =
        requirements_for_check(opts.root, opts.change).map_err(|e| e.to_string())?;
    let requirements = filter_requirements(all_requirements, &opts.plan.requirement_ids);
    // LemmaScript files whose proof could not be attempted because Dafny is
    // absent. Collected where the backend grouping happens, consumed where the
    // ledger outcomes are built.
    let mut dafny_unavailable_files: Vec<String> = Vec::new();
    let verified_config = load_verified_config(opts.root);
    let mapping_version = mapping_version_for_check(opts.root, opts.change);
    let verified_files = proof_eligible_files(&requirements, &verified_config, mapping_version);
    let mut phases = default_phases();
    mark_skipped_phases(&mut phases, &opts.plan);
    let mut issues: Vec<VerifyIssue> = Vec::new();
    let mut property_outcomes: Vec<crate::verify::ledger::PropertyOutcome> = Vec::new();

    if opts.plan.runs(CheckPhase::Structure) {
        opts.control.enter(CheckPhase::Structure)?;
        let config_path = workflow_config_read_path(opts.root);
        if !config_path.is_file() {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "structure".to_string(),
                requirement_id: None,
                message: ".forall/workflow/config.yaml missing (run `forall init`)".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let mapping_path = crate::paths::forall_mapping_path(opts.root);
        if !mapping_path.is_file() {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "structure".to_string(),
                requirement_id: None,
                message: ".forall/verify/mapping.yaml missing (run `forall init`)".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        if let Some(change) = opts.change {
            issues.extend(validate_change_properties(opts.root, change, &requirements));
        }
    } else {
        phases.insert("structure".to_string(), PhaseStatus::Skip);
    }

    let mut extracts = std::collections::HashMap::new();
    if opts.plan.runs(CheckPhase::Mapping)
        || opts.plan.runs(CheckPhase::Proofs)
        || opts.plan.runs(CheckPhase::Intent)
    {
        if opts.plan.runs(CheckPhase::Mapping) {
            opts.control.enter(CheckPhase::Mapping)?;
        }
        if opts.plan.runs(CheckPhase::Mapping) {
            issues.extend(validate_adapter_requirements(
                &requirements,
                &verified_config,
            ));
            issues.extend(validate_mapping_structure(opts.root, &requirements));
        }
        let lemmascript_files: Vec<String> =
            group_files_by_proof_backend(&verified_config, &verified_files)
                .get(PROOF_LEMMASCRIPT_DAFNY)
                .cloned()
                .unwrap_or_default();
        if opts.control.install_missing_toolchains
            && !lemmascript_files.is_empty()
            && !lsc_is_available()
        {
            let _ = ensure_lsc_toolchain(true, true);
        }
        // LemmaScript compiles to Dafny and shells out to it, so `lsc` being
        // present is only half the toolchain. Nothing used to install or even
        // look for Dafny, which is why its absence surfaced as a failed proof
        // blaming the user's contracts rather than as a missing prover.
        //
        // Gated on the proofs phase, not on this block. The block also covers
        // mapping and intent, and neither needs Dafny — contract extraction is
        // parsing. Reporting a missing prover during `--phase mapping` failed a
        // partial check over a phase it was told not to run.
        if opts.plan.runs(CheckPhase::Proofs)
            && opts.control.install_missing_toolchains
            && !lemmascript_files.is_empty()
            && !dafny_is_available()
        {
            let _ = ensure_dafny_toolchain(true, true);
        }
        if opts.plan.runs(CheckPhase::Proofs)
            && !lemmascript_files.is_empty()
            && !dafny_is_available()
        {
            dafny_unavailable_files = lemmascript_files.clone();
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message:
                    "Dafny proof engine not available for LemmaScript requirements; no proof was attempted"
                        .to_string(),
                file: lemmascript_files.first().cloned(),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Info,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: dafny_install_hint(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let (collected, extract_errors) =
            collect_extracts(opts.root, &verified_config, &verified_files);
        extracts = collected;
        if opts.plan.runs(CheckPhase::Mapping) {
            issues.extend(validate_symbols_against_extract(
                opts.root,
                &requirements,
                &extracts,
            ));
            for (file, err) in &extract_errors {
                phases.insert("mapping".to_string(), PhaseStatus::Fail);
                let hint = verified_config
                    .adapter_for_file(file)
                    .and_then(|(_, adapter)| backend_for_proof_id(&adapter.proof))
                    .map(super::backends::ProofBackend::install_hint)
                    .unwrap_or_else(lsc_install_hint);
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Critical,
                    phase: "mapping".to_string(),
                    requirement_id: None,
                    message: format!("proof engine failed on {file}: {}", lsc_error_detail(err)),
                    file: Some(file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Info,
                    phase: "mapping".to_string(),
                    requirement_id: None,
                    message: hint,
                    file: None,
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
            if issues
                .iter()
                .any(|i| i.phase == "mapping" && i.severity == IssueSeverity::Critical)
            {
                phases.insert("mapping".to_string(), PhaseStatus::Fail);
            }
        }
    } else {
        phases.insert("mapping".to_string(), PhaseStatus::Skip);
    }

    let mut proof_outcomes: Vec<ProofOutcome> = Vec::new();
    // Recorded distinctly from a failed proof: nothing was attempted, so the
    // ledger must not imply an obligation was checked and found wanting.
    if !dafny_unavailable_files.is_empty() {
        proof_outcomes.extend(dafny_unavailable_files.iter().map(|file| ProofOutcome {
            file: file.clone(),
            tool: PROOF_LEMMASCRIPT_DAFNY.to_string(),
            tool_version: None,
            status: ObligationStatus::ToolUnavailable,
        }));
    }

    if opts.plan.runs(CheckPhase::Proofs) {
        opts.control.enter(CheckPhase::Proofs)?;
        let verus_files: Vec<String> =
            group_files_by_proof_backend(&verified_config, &verified_files)
                .get(PROOF_VERUS)
                .cloned()
                .unwrap_or_default();
        if opts.control.install_missing_toolchains
            && !verus_files.is_empty()
            && !verus_is_available()
        {
            let _ = ensure_verus_toolchain(true, true);
        }
        if !verus_files.is_empty() && !verus_is_available() {
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: "Verus proof engine not available for verified Rust requirements"
                    .to_string(),
                file: verus_files.first().cloned(),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Info,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: verus_install_hint(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let verus_preflight_failed = !verus_files.is_empty() && !verus_is_available();

        let openjml_files: Vec<String> =
            group_files_by_proof_backend(&verified_config, &verified_files)
                .get(PROOF_OPENJML)
                .cloned()
                .unwrap_or_default();
        if opts.control.install_missing_toolchains
            && !openjml_files.is_empty()
            && !openjml_is_available()
        {
            let _ = ensure_openjml_toolchain(true, true);
        }
        if !openjml_files.is_empty() && !openjml_is_available() {
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: "OpenJML proof engine not available for verified Java requirements"
                    .to_string(),
                file: openjml_files.first().cloned(),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Info,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: openjml_install_hint(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let openjml_preflight_failed = !openjml_files.is_empty() && !openjml_is_available();

        let framac_files: Vec<String> =
            group_files_by_proof_backend(&verified_config, &verified_files)
                .get(PROOF_FRAMAC)
                .cloned()
                .unwrap_or_default();
        if opts.control.install_missing_toolchains
            && !framac_files.is_empty()
            && !framac_is_available()
        {
            let _ = ensure_framac_toolchain(true, true);
        }
        if !framac_files.is_empty() && !framac_is_available() {
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: "Frama-C proof engine not available for verified C requirements"
                    .to_string(),
                file: framac_files.first().cloned(),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Info,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: framac_install_hint(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let framac_preflight_failed = !framac_files.is_empty() && !framac_is_available();

        let lemmascript_proof_files: Vec<String> =
            group_files_by_proof_backend(&verified_config, &verified_files)
                .get(PROOF_LEMMASCRIPT_DAFNY)
                .cloned()
                .unwrap_or_default();
        if opts.control.install_missing_toolchains
            && !lemmascript_proof_files.is_empty()
            && !lsc_is_available()
        {
            let _ = ensure_lsc_toolchain(true, true);
        }
        if !lemmascript_proof_files.is_empty() && !lsc_is_available() {
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message:
                    "LemmaScript proof engine not available for verified TypeScript requirements"
                        .to_string(),
                file: lemmascript_proof_files.first().cloned(),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Info,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: lsc_install_hint(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
        let lsc_preflight_failed = !lemmascript_proof_files.is_empty() && !lsc_is_available();

        issues.extend(validate_dfy_merge_artifacts(
            opts.root,
            &lemmascript_proof_files,
        ));
        if issues
            .iter()
            .any(|i| i.phase == "proofs" && i.severity == IssueSeverity::Critical)
        {
            phases.insert("proofs".to_string(), PhaseStatus::Fail);
        }

        // A gap in tooling is recorded distinctly from a failed proof: the
        // ledger must never render "the prover was missing" as "the proof
        // failed".
        for (group, failed, tool) in [
            (&verus_files, verus_preflight_failed, PROOF_VERUS),
            (&openjml_files, openjml_preflight_failed, PROOF_OPENJML),
            (&framac_files, framac_preflight_failed, PROOF_FRAMAC),
            (
                &lemmascript_proof_files,
                lsc_preflight_failed,
                PROOF_LEMMASCRIPT_DAFNY,
            ),
        ] {
            if !failed {
                continue;
            }
            proof_outcomes.extend(group.iter().map(|file| ProofOutcome {
                file: file.clone(),
                tool: tool.to_string(),
                tool_version: None,
                status: ObligationStatus::ToolUnavailable,
            }));
        }

        for (proof_id, files) in group_files_by_proof_backend(&verified_config, &verified_files) {
            if proof_id == PROOF_VERUS && verus_preflight_failed {
                continue;
            }
            if proof_id == PROOF_OPENJML && openjml_preflight_failed {
                continue;
            }
            if proof_id == PROOF_FRAMAC && framac_preflight_failed {
                continue;
            }
            // `lsc` present but Dafny absent means the proof cannot be
            // attempted. Running it anyway produced a second, contradictory
            // pair of issues telling the user to fix contracts that are fine —
            // the exact blame this change exists to remove.
            if proof_id == PROOF_LEMMASCRIPT_DAFNY
                && (lsc_preflight_failed || !dafny_unavailable_files.is_empty())
            {
                continue;
            }
            let Some(backend) = backend_for_proof_id(&proof_id) else {
                continue;
            };
            let toolchain_available = match proof_id.as_str() {
                PROOF_VERUS => verus_is_available(),
                PROOF_OPENJML => openjml_is_available(),
                PROOF_FRAMAC => framac_is_available(),
                PROOF_LEMMASCRIPT_DAFNY => lsc_is_available() && dafny_is_available(),
                _ => false,
            };
            for mut output in backend.run_check(opts.root, &files) {
                if output.result.tool_version.is_none() {
                    output.result.tool_version = backend.tool_version();
                }
                let status = if output.result.ok {
                    ObligationStatus::Proved
                } else {
                    ObligationStatus::Failed
                };
                proof_outcomes.extend(files_covered_by_output(&output.label, &files).iter().map(
                    |file| ProofOutcome {
                        file: (*file).to_string(),
                        tool: backend.id().to_string(),
                        tool_version: output.result.tool_version.clone(),
                        status,
                    },
                ));
                if output.result.ok {
                    continue;
                }
                phases.insert("proofs".to_string(), PhaseStatus::Fail);
                let cited = requirement_ids_for_output(&requirements, &output.label, &files);
                issues.extend(proof_failure_issues(
                    backend,
                    &output,
                    toolchain_available,
                    &cited,
                ));
            }
        }
    } else {
        phases.insert("proofs".to_string(), PhaseStatus::Skip);
    }

    if opts.plan.runs(CheckPhase::Intent) {
        opts.control.enter(CheckPhase::Intent)?;
        let (intent_issues, records) = validate_intent(opts.root, &requirements, &extracts);
        issues.extend(intent_issues);
        phases.insert(
            "intent".to_string(),
            intent_phase_status(&requirements, &issues),
        );
        let _ = write_claimcheck_records(opts.root, opts.change, &records);
    } else {
        phases.insert("intent".to_string(), PhaseStatus::Skip);
    }

    if opts.plan.runs(CheckPhase::Scenarios) {
        opts.control.enter(CheckPhase::Scenarios)?;
        issues.extend(validate_scenarios(opts.root, opts.change, &requirements));
        phases.insert(
            "scenarios".to_string(),
            scenarios_phase_status(&requirements, &issues),
        );
    } else {
        phases.insert("scenarios".to_string(), PhaseStatus::Skip);
    }

    if opts.plan.runs(CheckPhase::PropertyTests) {
        opts.control.enter(CheckPhase::PropertyTests)?;
        issues.extend(validate_property_files(
            opts.root,
            &requirements,
            mapping_version,
        ));
        let pbt_options = PropertyTestOptions {
            seed: opts.pbt_seed,
            examples: opts.pbt_examples,
        };
        let property_run =
            run_property_tests(opts.root, &requirements, &pbt_options, mapping_version);
        property_outcomes = property_run.outcomes;
        issues.extend(property_run.issues);
        phases.insert(
            "property-tests".to_string(),
            property_tests_phase_status(&requirements, &issues, mapping_version),
        );
    } else {
        phases.insert("property-tests".to_string(), PhaseStatus::Skip);
    }

    let mut scenario_outcomes: Vec<crate::verify::ledger::ScenarioOutcome> = Vec::new();
    if opts.plan.runs(CheckPhase::ScenarioTests) {
        opts.control.enter(CheckPhase::ScenarioTests)?;
        let scenario_run = run_scenario_tests(opts.root, &requirements);
        scenario_outcomes = scenario_run.outcomes;
        issues.extend(scenario_run.issues);
        phases.insert(
            "scenario-tests".to_string(),
            scenario_tests_phase_status(opts.root, &requirements, &issues),
        );
    } else {
        phases.insert("scenario-tests".to_string(), PhaseStatus::Skip);
    }

    // The ledger is only written when the proofs phase actually ran: a
    // structure-only check must not overwrite a rich ledger with an empty
    // one. Best-effort like the claimcheck records — a ledger write failure
    // must not fail the check itself.
    // Any evidence-producing phase writes the ledger, not just proofs: a
    // property-only run has outcomes to record too.
    let ran_proofs = opts.plan.runs(CheckPhase::Proofs);
    let ran_properties = opts.plan.runs(CheckPhase::PropertyTests);
    let ran_scenarios = opts.plan.runs(CheckPhase::ScenarioTests);
    let ledger = if ran_proofs || ran_properties || ran_scenarios {
        let built = build_ledger_with_evidence(
            opts.root,
            opts.change,
            &requirements,
            &proof_outcomes,
            &property_outcomes,
            &scenario_outcomes,
        );
        // A requirement-filtered run (the daemon's incremental path) sees
        // only a slice of the mapping; folding it into the existing ledger
        // keeps every unaffected entry. Only a full run replaces wholesale —
        // which is also what prunes entries for deleted requirements.
        // Two merges, in order: phase-aware (keep evidence from phases
        // that did not run), then requirement-aware (keep entries this
        // filtered run never looked at).
        let previous = read_ledger(opts.root, opts.change);
        let built = merge_run(
            previous.clone(),
            built,
            &requirements,
            ran_proofs,
            ran_properties,
            ran_scenarios,
        );
        let merged = if opts
            .plan
            .requirement_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty())
        {
            merge_partial(previous, built)
        } else {
            built
        };
        let _ = write_ledger(opts.root, &merged);
        Some(merged)
    } else {
        // Phases that skip proofs still summarize from the last run's ledger.
        read_ledger(opts.root, opts.change)
    };

    // Version-2 mappings have no claim booleans, so their summary counts are
    // derived from the ledger's earned levels — the whole point of the split.
    // Version-1 mappings keep their legacy claim-derived summary.
    // Derived from the ledger for every mapping version, not just version 2.
    // The version-1 path counted requirements whose `verified` bool was set,
    // which is a claim the author wrote rather than an outcome any tool
    // produced — so a project whose proofs all failed still reported them as
    // proved. summarize_earned reads the level each requirement actually
    // earned, and reports nothing as proved when there is no ledger at all.
    let verification_summary = summarize_earned(ledger.as_ref(), requirements.len());
    if opts.change.is_some() && requirements.is_empty() {
        issues.push(VerifyIssue {
            severity: IssueSeverity::Warning,
            phase: "mapping".to_string(),
            requirement_id: None,
            message: "Change-scoped check found no requirements in mapping.delta.yaml — add requirements before archive.".to_string(),
            file: None,
            counterexample: None,
            pbt: None,
            proof_detail: None,
        });
    }
    if opts.strict
        && verification_summary.total_requirements > 0
        && verification_summary.proved_requirements == 0
        && opts.plan.runs(CheckPhase::Proofs)
    {
        let message = if let Some(change) = opts.change {
            format!(
                "Change '{change}' has no proved requirements — check passed on workflow/mapping only (spec-tracked, not formally proved)."
            )
        } else {
            "No requirements are proved — check passed on workflow/mapping only (spec-tracked, not formally proved).".to_string()
        };
        issues.push(VerifyIssue {
            severity: IssueSeverity::Warning,
            phase: "proofs".to_string(),
            requirement_id: None,
            message,
            file: None,
            counterexample: None,
            pbt: None,
            proof_detail: None,
        });
    }

    let has_critical = issues.iter().any(|i| i.severity == IssueSeverity::Critical);
    Ok(VerifyReport {
        ok: !has_critical,
        strict: opts.strict,
        root: opts.root.display().to_string(),
        change: opts.change.map(str::to_string),
        phases,
        issues,
        verified_files,
        verification_summary,
    })
}

#[cfg(test)]
mod proof_failure_attribution_tests {
    use super::*;
    use crate::doctor::{WorkflowDoctorCheck, WorkflowDoctorStatus};
    use crate::mapping::validate::ExtractResult;
    use crate::verify::backends::BackendRunResult;

    struct StubBackend;

    impl ProofBackend for StubBackend {
        fn id(&self) -> &'static str {
            "stub"
        }

        fn tool_version(&self) -> Option<String> {
            Some("9.9-test".to_string())
        }

        fn extract_contracts(
            &self,
            _root: &Path,
            _file: &str,
        ) -> (Option<ExtractResult>, Option<BackendRunResult>) {
            (None, None)
        }

        fn run_check(&self, _root: &Path, files: &[String]) -> Vec<BackendCheckOutput> {
            files
                .iter()
                .enumerate()
                .map(|(idx, file)| BackendCheckOutput {
                    label: file.clone(),
                    result: BackendRunResult {
                        ok: idx == 0,
                        code: Some(if idx == 0 { 0 } else { 1 }),
                        stdout: String::new(),
                        stderr: format!("proof failed in {file}"),
                        command: format!("stub verify {file}"),
                        timed_out: false,
                        tool_version: Some("9.9-test".to_string()),
                    },
                })
                .collect()
        }

        fn run_sync(&self, _root: &Path, _file: &str) -> BackendRunResult {
            BackendRunResult {
                ok: true,
                code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                command: "stub sync".to_string(),
                timed_out: false,
                tool_version: Some("9.9-test".to_string()),
            }
        }

        fn install_hint(&self) -> String {
            "install stub".to_string()
        }

        fn error_detail(&self, result: &BackendRunResult) -> String {
            result.stderr.clone()
        }

        fn doctor_check(&self) -> WorkflowDoctorCheck {
            WorkflowDoctorCheck {
                id: "stub".to_string(),
                status: WorkflowDoctorStatus::Ok,
                summary: "stub".to_string(),
                details: Vec::new(),
                remediation: None,
            }
        }
    }

    #[test]
    fn proof_failure_attributes_issue_to_failing_file_not_first_in_group() {
        let backend = StubBackend;
        let files = vec!["src/first.dfy".to_string(), "src/second.dfy".to_string()];

        let failing: Vec<BackendCheckOutput> = backend
            .run_check(Path::new("/tmp/project"), &files)
            .into_iter()
            .filter(|output| !output.result.ok)
            .collect();
        assert_eq!(failing.len(), 1);

        let [critical, hint] = proof_failure_issues(&backend, &failing[0], true, &[]);

        assert_eq!(critical.file.as_deref(), Some("src/second.dfy"));
        assert_eq!(hint.file.as_deref(), Some("src/second.dfy"));
        assert_eq!(critical.severity, IssueSeverity::Critical);
        assert!(critical.message.contains("src/second.dfy"));
        assert!(
            critical
                .proof_detail
                .as_deref()
                .is_some_and(|d| d.contains("stub verify src/second.dfy"))
        );
    }

    #[test]
    fn proof_failure_carries_tool_identity_in_detail() {
        let backend = StubBackend;
        let files = vec!["src/a.dfy".to_string(), "src/b.dfy".to_string()];
        let failing: Vec<BackendCheckOutput> = backend
            .run_check(Path::new("/tmp/project"), &files)
            .into_iter()
            .filter(|output| !output.result.ok)
            .collect();

        let [critical, _] = proof_failure_issues(&backend, &failing[0], true, &[]);

        let detail = critical.proof_detail.as_deref().unwrap();
        assert!(detail.starts_with("tool: stub 9.9-test\n"));
        assert!(detail.contains("command: stub verify"));
    }

    #[test]
    fn sole_requirement_is_attributed_and_multiple_stay_in_message() {
        let backend = StubBackend;
        let files = vec!["src/a.dfy".to_string(), "src/b.dfy".to_string()];
        let failing: Vec<BackendCheckOutput> = backend
            .run_check(Path::new("/tmp/project"), &files)
            .into_iter()
            .filter(|output| !output.result.ok)
            .collect();

        let [critical, hint] = proof_failure_issues(&backend, &failing[0], true, &["REQ-7"]);
        assert_eq!(critical.requirement_id.as_deref(), Some("REQ-7"));
        assert_eq!(hint.requirement_id.as_deref(), Some("REQ-7"));
        assert!(critical.message.contains("(requirements: REQ-7)"));

        let [critical, hint] =
            proof_failure_issues(&backend, &failing[0], true, &["REQ-7", "REQ-9"]);
        assert_eq!(critical.requirement_id, None);
        assert_eq!(hint.requirement_id, None);
        assert!(critical.message.contains("(requirements: REQ-7, REQ-9)"));
    }

    #[test]
    fn crate_labelled_output_claims_only_its_own_group() {
        use crate::mapping::schema::CodeRef;

        let req = |id: &str, file: &str| Requirement {
            id: id.to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: file.to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        };
        let requirements = vec![
            req("REQ-lib", "crates/lib/src/a.rs"),
            req("REQ-app", "crates/app/src/b.rs"),
            req("REQ-c", "src/frame.c"),
        ];
        let rust_group = vec![
            "crates/lib/src/a.rs".to_string(),
            "crates/app/src/b.rs".to_string(),
        ];

        // Verus labels the crate directory: it claims the files under it...
        assert_eq!(
            requirement_ids_for_output(&requirements, "crates/lib", &rust_group),
            vec!["REQ-lib"]
        );
        // ...and at the repo root, every file in its own group — never the C
        // file, which belongs to another backend's group.
        assert_eq!(
            requirement_ids_for_output(&requirements, "", &rust_group),
            vec!["REQ-lib", "REQ-app"]
        );
        // A sibling directory sharing a name prefix is not "under" the label.
        assert!(
            requirement_ids_for_output(&requirements, "crates/li", &rust_group).is_empty(),
            "prefix match must respect path boundaries"
        );
    }

    #[test]
    fn requirement_join_matches_on_code_file() {
        use crate::mapping::schema::CodeRef;

        let req = |id: &str, file: &str| Requirement {
            id: id.to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: file.to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        };
        let unmapped = Requirement {
            code: None,
            ..req("REQ-none", "unused")
        };
        let requirements = vec![req("REQ-1", "src/a.c"), req("REQ-2", "src/a.c"), unmapped];

        assert_eq!(
            requirement_ids_for_output(&requirements, "src/a.c", &["src/a.c".to_string()]),
            vec!["REQ-1", "REQ-2"]
        );
        assert!(
            requirement_ids_for_output(&requirements, "src/z.c", &["src/a.c".to_string()])
                .is_empty()
        );
    }
}

#[cfg(test)]
mod hosted_control_tests {
    use std::cell::RefCell;

    use super::*;

    fn project() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("temp project");
        let verify = temp.path().join(".forall/verify");
        std::fs::create_dir_all(&verify).expect("verify directory");
        std::fs::write(
            verify.join("mapping.yaml"),
            "version: 1\nrequirements: []\n",
        )
        .expect("mapping");
        temp
    }

    #[test]
    fn hosted_control_disables_install_and_reports_phase() {
        let temp = project();
        let entered = RefCell::new(Vec::new());
        let on_phase = |phase| entered.borrow_mut().push(phase);
        let control = CheckExecutionControl::hosted(Some(&on_phase), None);
        assert!(!control.install_missing_toolchains);

        let report = execute_check(CheckExecutionOptions {
            root: temp.path(),
            change: None,
            strict: false,
            pbt_seed: 42,
            pbt_examples: 10,
            plan: PhasePlan {
                phases: BTreeSet::from([CheckPhase::Structure]),
                requirement_ids: None,
            },
            control,
        })
        .expect("hosted check");

        assert!(report.ok);
        assert_eq!(*entered.borrow(), vec![CheckPhase::Structure]);
    }

    #[test]
    fn hosted_control_cancels_before_entering_phase() {
        let temp = project();
        let error = execute_check(CheckExecutionOptions {
            root: temp.path(),
            change: None,
            strict: false,
            pbt_seed: 42,
            pbt_examples: 10,
            plan: PhasePlan {
                phases: BTreeSet::from([CheckPhase::Structure]),
                requirement_ids: None,
            },
            control: CheckExecutionControl::hosted(None, Some(&|| true)),
        })
        .expect_err("cancelled check");

        assert_eq!(error, "verification cancelled");
    }
}
