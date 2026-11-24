use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use crate::mapping::schema::{IssueSeverity, PbtRunMeta, Requirement, VerifyIssue};
use crate::paths::FORALL_DIR;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PropertyTestResult {
    ok: bool,
    #[serde(default)]
    counterexample: Option<serde_json::Value>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    examples_run: Option<u32>,
    #[serde(default)]
    message: Option<String>,
    /// The runner could not load the module's dependencies, so no property
    /// was ever evaluated. Distinct from `ok: false`, which means one was
    /// evaluated and found false.
    #[serde(default)]
    tool_unavailable: bool,
}

pub struct PropertyTestOptions {
    pub seed: u64,
    pub examples: u32,
}

fn run_property_runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/run-property.mjs")
}

fn run_python_property_runner() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/run-property.py")
}

fn is_python_property(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("py")
}

/// Node 22+ can load `.ts` via `--experimental-strip-types` without tsx IPC pipes,
/// which seatbelt/macOS sandboxes block (tsx uses a temp-dir named pipe).
fn node_supports_strip_types(node: &str) -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut command = Command::new(node);
        command.args(["--experimental-strip-types", "-e", ""]);
        crate::verify::command_security::apply_hosted_identity(&mut command);
        command
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
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

fn node_ts_loader_args(node: &str) -> Vec<String> {
    if node_supports_strip_types(node) {
        return vec![String::from("--experimental-strip-types")];
    }
    tsx_loader_args()
}

/// Whether a requirement intends property testing.
///
/// Version-1 mappings say so with the writable `property_tested` bool, and
/// say it in both directions: `property_tested: false` beside an attached
/// property file is a disarm, so there the bool stays authoritative. Version
/// 2 has no claim bools at all, so intent is the artifact: an attached
/// property file. Gating on the bool alone silently skipped the whole phase
/// for every v2 project — the tooling was there, the gate never opened.
pub fn intends_property_test(req: &Requirement, version: u32) -> bool {
    if version >= 2 {
        req.property.is_some()
    } else {
        req.property_tested
    }
}

pub fn property_test_path(root: &Path, req: &Requirement, version: u32) -> Option<PathBuf> {
    if !intends_property_test(req, version) {
        return None;
    }
    if let Some(prop) = &req.property {
        let candidate = root.join(&prop.file);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let default = root
        .join(FORALL_DIR)
        .join("scenarios")
        .join(format!("{}.property.ts", req.id));
    if default.is_file() {
        Some(default)
    } else {
        None
    }
}

pub fn validate_property_files(
    root: &Path,
    requirements: &[Requirement],
    version: u32,
) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();
    for req in requirements {
        if !intends_property_test(req, version) {
            continue;
        }
        if req.verified {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "mapping".to_string(),
                requirement_id: Some(req.id.clone()),
                message: "requirement cannot be both verified and property_tested".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            continue;
        }
        if property_test_path(root, req, version).is_none() {
            let expected = req
                .property
                .as_ref()
                .map(|p| p.file.as_str())
                .unwrap_or(".forall/scenarios/{id}.property.ts");
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "mapping".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!(
                    "property_tested requirement missing property file (expected {expected})"
                ),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
    }
    issues
}

/// Issues plus per-requirement outcomes. Outcomes are recorded for PASSING
/// runs too — a silent success is exactly why the ledger could never grant
/// PropertyTested.
pub struct PropertyTestRun {
    pub issues: Vec<VerifyIssue>,
    pub outcomes: Vec<crate::verify::ledger::PropertyOutcome>,
}

pub fn run_property_tests(
    root: &Path,
    requirements: &[Requirement],
    options: &PropertyTestOptions,
    version: u32,
) -> PropertyTestRun {
    run_property_tests_detailed(root, requirements, options, version)
}

pub fn run_property_tests_detailed(
    root: &Path,
    requirements: &[Requirement],
    options: &PropertyTestOptions,
    version: u32,
) -> PropertyTestRun {
    let mut issues = Vec::new();
    let mut outcomes: Vec<crate::verify::ledger::PropertyOutcome> = Vec::new();
    let ts_runner = run_property_runner();
    let py_runner = run_python_property_runner();

    let node = env::var("NODE").unwrap_or_else(|_| "node".to_string());
    let python = env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());

    for req in requirements {
        if !intends_property_test(req, version) {
            continue;
        }
        let Some(test_path) = property_test_path(root, req, version) else {
            continue;
        };

        let rel = test_path
            .strip_prefix(root)
            .unwrap_or(&test_path)
            .display()
            .to_string();

        let output = if is_python_property(&test_path) {
            if !py_runner.is_file() {
                continue;
            }
            let mut command = Command::new(&python);
            command
                .arg(&py_runner)
                .arg(test_path.display().to_string())
                .current_dir(root)
                .env("FORALL_PBT_SEED", options.seed.to_string())
                .env("FORALL_PBT_EXAMPLES", options.examples.to_string());
            crate::verify::command_security::apply_hosted_identity(&mut command);
            command.output()
        } else {
            if !ts_runner.is_file() {
                continue;
            }
            let mut args = node_ts_loader_args(&node);
            args.push(ts_runner.display().to_string());
            args.push(test_path.display().to_string());
            let mut command = Command::new(&node);
            command
                .args(&args)
                .current_dir(root)
                .env("FORALL_PBT_SEED", options.seed.to_string())
                .env("FORALL_PBT_EXAMPLES", options.examples.to_string());
            crate::verify::command_security::apply_hosted_identity(&mut command);
            command.output()
        };

        let Ok(out) = output else {
            let runtime = if is_python_property(&test_path) {
                "python"
            } else {
                "node"
            };
            // The runtime is absent, so nothing ran. Record the gap rather
            // than a verdict: the ledger must never say a property was
            // falsified by a run that never happened.
            outcomes.push(crate::verify::ledger::PropertyOutcome {
                requirement_id: req.id.clone(),
                file: rel.clone(),
                status: crate::verify::ledger::ObligationStatus::ToolUnavailable,
                seed: options.seed,
                examples_run: 0,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "property-tests".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!(
                    "property test for {} did not run: {runtime} is not available",
                    req.id
                ),
                file: Some(rel),
                counterexample: None,
                pbt: Some(PbtRunMeta {
                    seed: options.seed,
                    examples_run: options.examples,
                }),
                proof_detail: None,
            });
            continue;
        };

        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout_empty = stdout.trim().is_empty();
        let stderr_trimmed = stderr.trim().to_string();
        let parsed: PropertyTestResult =
            serde_json::from_str(stdout.trim()).unwrap_or(PropertyTestResult {
                ok: false,
                counterexample: None,
                seed: Some(options.seed),
                examples_run: Some(options.examples),
                message: Some(if stdout_empty {
                    stderr_trimmed.clone()
                } else {
                    stdout.into_owned()
                }),
                tool_unavailable: false,
            });

        let seed = parsed.seed.unwrap_or(options.seed);
        let examples_run = parsed.examples_run.unwrap_or(options.examples);
        let pbt = PbtRunMeta { seed, examples_run };

        if parsed.tool_unavailable {
            outcomes.push(crate::verify::ledger::PropertyOutcome {
                requirement_id: req.id.clone(),
                file: rel.clone(),
                status: crate::verify::ledger::ObligationStatus::ToolUnavailable,
                seed,
                examples_run: 0,
            });
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "property-tests".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!(
                    "property test for {} did not run: {}",
                    req.id,
                    parsed
                        .message
                        .as_deref()
                        .unwrap_or("a dependency could not be resolved")
                ),
                file: Some(rel.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            continue;
        }

        if parsed.ok && out.status.success() {
            outcomes.push(crate::verify::ledger::PropertyOutcome {
                requirement_id: req.id.clone(),
                file: rel.clone(),
                status: crate::verify::ledger::ObligationStatus::Proved,
                seed,
                examples_run,
            });
            continue;
        }
        outcomes.push(crate::verify::ledger::PropertyOutcome {
            requirement_id: req.id.clone(),
            file: rel.clone(),
            status: crate::verify::ledger::ObligationStatus::Failed,
            seed,
            examples_run,
        });

        let detail = parsed.message.filter(|m| !m.is_empty()).unwrap_or_else(|| {
            if !parsed.ok {
                "property test returned ok: false".to_string()
            } else {
                format!(
                    "property test exited {}",
                    out.status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )
            }
        });
        let detail = if !stderr_trimmed.is_empty()
            && !detail.contains(&stderr_trimmed)
            && (stdout_empty || !parsed.ok)
        {
            format!("{detail} ({stderr_trimmed})")
        } else {
            detail
        };

        issues.push(VerifyIssue {
            severity: IssueSeverity::Critical,
            phase: "property-tests".to_string(),
            requirement_id: Some(req.id.clone()),
            message: format!("property test failed for {}: {detail}", req.id),
            file: Some(rel),
            counterexample: parsed.counterexample,
            pbt: Some(pbt),
            proof_detail: None,
        });
    }

    PropertyTestRun { issues, outcomes }
}

pub fn property_tests_phase_status(
    requirements: &[Requirement],
    issues: &[VerifyIssue],
    version: u32,
) -> crate::mapping::schema::PhaseStatus {
    use crate::mapping::schema::PhaseStatus;
    let has_property = requirements
        .iter()
        .any(|r| intends_property_test(r, version));
    if !has_property {
        return PhaseStatus::Skip;
    }
    if issues
        .iter()
        .any(|i| i.phase == "property-tests" && i.severity == IssueSeverity::Critical)
    {
        return PhaseStatus::Fail;
    }
    PhaseStatus::Pass
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::PropertyRef;

    #[test]
    fn resolves_default_property_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let scenarios = root.join(FORALL_DIR).join("scenarios");
        std::fs::create_dir_all(&scenarios).expect("mkdir");
        std::fs::write(
            scenarios.join("clamp-bounds.property.ts"),
            "export default async () => ({ ok: true });",
        )
        .expect("write");
        let req = Requirement {
            id: "clamp-bounds".to_string(),
            capability: "clamp".to_string(),
            requirement: "bounds".to_string(),
            verified: false,
            property_tested: true,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        };
        assert!(property_test_path(root, &req, 1).is_some());
    }

    /// The runners signal "the check never happened" separately from "the
    /// property is false". Without that, a project missing fast-check or
    /// Hypothesis had a falsified property written into its ledger.
    #[test]
    fn a_missing_dependency_is_parsed_as_a_gap_not_a_falsification() {
        let unavailable: PropertyTestResult = serde_json::from_str(
            r#"{"ok":false,"toolUnavailable":true,"message":"Cannot find package 'fast-check'"}"#,
        )
        .expect("runner payload");
        assert!(unavailable.tool_unavailable);
        assert!(!unavailable.ok);

        // A real counterexample must not be mistaken for a missing tool.
        let falsified: PropertyTestResult = serde_json::from_str(
            r#"{"ok":false,"counterexample":[0,0],"seed":42,"examplesRun":100}"#,
        )
        .expect("runner payload");
        assert!(!falsified.tool_unavailable);
        assert_eq!(falsified.seed, Some(42));

        // Nor an error in the author's own property file.
        let author_bug: PropertyTestResult = serde_json::from_str(
            r#"{"ok":false,"toolUnavailable":false,"message":"Unexpected identifier"}"#,
        )
        .expect("runner payload");
        assert!(!author_bug.tool_unavailable);
    }

    /// Payloads written before the field existed must keep meaning what they
    /// meant: a verdict, not a gap.
    #[test]
    fn a_payload_without_the_field_is_still_a_verdict() {
        let legacy: PropertyTestResult =
            serde_json::from_str(r#"{"ok":true,"seed":7,"examplesRun":100}"#).expect("payload");
        assert!(!legacy.tool_unavailable);
        assert!(legacy.ok);
    }

    /// A version-2 requirement carries no `property_tested` bool — the
    /// attached property file is the whole claim. Gating on the bool made
    /// the phase skip silently for every migrated project.
    #[test]
    fn an_attached_property_file_opens_the_gate_without_the_bool() {
        let req = Requirement {
            id: "split-never-exceeds-total".to_string(),
            capability: "split".to_string(),
            requirement: "shares never exceed the total".to_string(),
            verified: false,
            property_tested: false,
            property: Some(PropertyRef {
                file: ".forall/scenarios/split.property.ts".to_string(),
                symbol: None,
            }),
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        };

        assert!(intends_property_test(&req, 2));

        // In a version-1 mapping the bool still speaks in both directions:
        // an attached file with `property_tested: false` is a disarm, the
        // mirror of `verified: false` beside a finished contract.
        assert!(!intends_property_test(&req, 1));
    }

    #[test]
    fn validates_missing_property_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let req = Requirement {
            id: "clamp-bounds".to_string(),
            capability: "clamp".to_string(),
            requirement: "bounds".to_string(),
            verified: false,
            property_tested: true,
            property: Some(PropertyRef {
                file: ".forall/scenarios/clamp-bounds.property.ts".to_string(),
                symbol: Some("clamp".to_string()),
            }),
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        };
        let issues = validate_property_files(dir.path(), &[req], 1);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].phase, "mapping");
    }
}
