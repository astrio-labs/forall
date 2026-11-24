use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::diagnostic::format_diagnostic_output;
use crate::mapping::schema::{IssueSeverity, Requirement, VerifyIssue};
use crate::paths::scenarios_dir as forall_scenarios_dir;
use crate::verify::ledger::{ObligationStatus, ScenarioOutcome};

const SCENARIO_SUFFIX: &str = ".scenario.mjs";

fn scenarios_dir(root: &Path) -> std::path::PathBuf {
    forall_scenarios_dir(root)
}

fn scenario_test_path(root: &Path, requirement_id: &str) -> std::path::PathBuf {
    scenarios_dir(root).join(format!("{requirement_id}{SCENARIO_SUFFIX}"))
}

fn tsx_loader_args() -> Vec<String> {
    let tsx_cli = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("node_modules")
        .join("tsx")
        .join("dist")
        .join("cli.mjs");
    if tsx_cli.is_file() {
        return vec![String::from("--import"), tsx_cli.display().to_string()];
    }
    vec![String::from("--import"), String::from("tsx")]
}

/// Issues plus per-requirement outcomes, mirroring [`PropertyTestRun`].
///
/// Outcomes are recorded for PASSING runs too. This phase used to report only
/// failures, so a passing scenario left no trace and `Tested` (E2) was a rung
/// nothing could ever earn — the same silent-success gap that was already
/// closed for property runs.
///
/// [`PropertyTestRun`]: crate::verify::property_tests::PropertyTestRun
pub struct ScenarioTestRun {
    pub issues: Vec<VerifyIssue>,
    pub outcomes: Vec<ScenarioOutcome>,
}

pub fn run_scenario_tests(root: &Path, requirements: &[Requirement]) -> ScenarioTestRun {
    let mut issues = Vec::new();
    let mut outcomes: Vec<ScenarioOutcome> = Vec::new();
    let dir = scenarios_dir(root);
    if !dir.is_dir() {
        return ScenarioTestRun { issues, outcomes };
    }

    let node = env::var("NODE").unwrap_or_else(|_| "node".to_string());

    for req in requirements {
        let test_path = scenario_test_path(root, &req.id);
        if !test_path.is_file() {
            continue;
        }

        let mut args = tsx_loader_args();
        args.push(test_path.display().to_string());

        let mut command = Command::new(&node);
        command.args(&args).current_dir(root);
        crate::verify::command_security::apply_hosted_identity(&mut command);
        let output = command.output();

        let rel_test_path = test_path
            .strip_prefix(root)
            .unwrap_or(&test_path)
            .display()
            .to_string();

        let Ok(out) = output else {
            // The runner never ran, which is a gap in tooling rather than a
            // failing scenario, and must not read as one.
            outcomes.push(ScenarioOutcome {
                requirement_id: req.id.clone(),
                file: rel_test_path.clone(),
                status: ObligationStatus::ToolUnavailable,
            });
            issues.push(VerifyIssue {
                severity: if req.verified {
                    IssueSeverity::Critical
                } else {
                    IssueSeverity::Warning
                },
                phase: "scenario-tests".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!("scenario test failed for {}: could not spawn node", req.id),
                file: Some(
                    test_path
                        .strip_prefix(root)
                        .unwrap_or(&test_path)
                        .display()
                        .to_string(),
                ),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            continue;
        };

        if out.status.success() {
            outcomes.push(ScenarioOutcome {
                requirement_id: req.id.clone(),
                file: rel_test_path.clone(),
                status: ObligationStatus::Proved,
            });
            continue;
        }

        outcomes.push(ScenarioOutcome {
            requirement_id: req.id.clone(),
            file: rel_test_path.clone(),
            status: ObligationStatus::Failed,
        });

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let combined = [stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let formatted = if combined.is_empty() {
            String::new()
        } else {
            format_diagnostic_output(&combined)
        };

        let message = if formatted.is_empty() {
            format!(
                "scenario test failed for {} (exit {})",
                req.id,
                out.status
                    .code()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )
        } else {
            format!("scenario test failed for {}: {formatted}", req.id)
        };

        issues.push(VerifyIssue {
            severity: if req.verified {
                IssueSeverity::Critical
            } else {
                IssueSeverity::Warning
            },
            phase: "scenario-tests".to_string(),
            requirement_id: Some(req.id.clone()),
            message,
            file: Some(
                test_path
                    .strip_prefix(root)
                    .unwrap_or(&test_path)
                    .display()
                    .to_string(),
            ),
            counterexample: None,
            pbt: None,
            proof_detail: None,
        });
    }

    ScenarioTestRun { issues, outcomes }
}

pub fn scenario_tests_phase_status(
    root: &Path,
    requirements: &[Requirement],
    issues: &[VerifyIssue],
) -> crate::mapping::schema::PhaseStatus {
    use crate::mapping::schema::PhaseStatus;
    let has_tests = requirements
        .iter()
        .any(|req| scenario_test_path(root, &req.id).is_file());
    if !has_tests {
        return PhaseStatus::Skip;
    }
    if issues
        .iter()
        .any(|i| i.phase == "scenario-tests" && i.severity == IssueSeverity::Critical)
    {
        return PhaseStatus::Fail;
    }
    if issues.iter().any(|i| i.phase == "scenario-tests") {
        return PhaseStatus::Fail;
    }
    PhaseStatus::Pass
}
