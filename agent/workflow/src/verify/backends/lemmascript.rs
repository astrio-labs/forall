use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use crate::diagnostic::format_diagnostic_output;
use crate::doctor::{WorkflowDoctorCheck, WorkflowDoctorStatus};
use crate::mapping::validate::ExtractResult;
use crate::verify::adapters::PROOF_LEMMASCRIPT_DAFNY;

use super::lsc_install;
use super::proof_runner::{self, lsc_proof_timeout_secs};
use super::{BackendCheckOutput, BackendRunResult, ProofBackend};

pub type LscRunResult = BackendRunResult;

fn lsc_script_at(base_dir: &Path) -> Option<PathBuf> {
    let candidate = base_dir
        .join("node_modules")
        .join("lemmascript")
        .join("tools")
        .join("dist")
        .join("lsc.js");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn find_bundled_lsc() -> Option<PathBuf> {
    let workflow_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(from_workflow) = lsc_script_at(&workflow_root) {
        return Some(from_workflow);
    }

    let agent_root = workflow_root.join("..");
    if let Some(from_agent) = lsc_script_at(&agent_root) {
        return Some(from_agent);
    }

    let mut dir = env::current_dir().ok()?;
    loop {
        if let Some(found) = lsc_script_at(&dir) {
            return Some(found);
        }
        let parent = dir.parent()?;
        if parent == dir {
            break;
        }
        dir = parent.to_path_buf();
    }

    if let Ok(home) = env::var("HOME") {
        let home_lemmascript = PathBuf::from(home)
            .join("Projects")
            .join("LemmaScript")
            .join("tools")
            .join("dist")
            .join("lsc.js");
        if home_lemmascript.is_file() {
            return Some(home_lemmascript);
        }
    }

    None
}

pub fn resolve_lsc_command() -> (String, Vec<String>) {
    if let Some(explicit) = lsc_install::configured_lsc_override() {
        let parts: Vec<&str> = explicit.split_whitespace().collect();
        if let Some((cmd, rest)) = parts.split_first() {
            return (
                cmd.to_string(),
                rest.iter().map(|s| (*s).to_string()).collect(),
            );
        }
    }

    if let Some(forall_script) = lsc_install::available_npm_script() {
        let node = env::var("NODE").unwrap_or_else(|_| "node".to_string());
        return (node, vec![forall_script.display().to_string()]);
    }

    if let Some(bundled) = find_bundled_lsc() {
        let node = env::var("NODE").unwrap_or_else(|_| "node".to_string());
        return (node, vec![bundled.display().to_string()]);
    }

    (
        "npx".to_string(),
        vec!["--yes".to_string(), "lemmascript".to_string()],
    )
}

pub fn lsc_install_hint() -> String {
    lsc_install::lsc_install_hint()
}

pub fn lsc_proof_failure_hint(result: &BackendRunResult) -> String {
    if result.timed_out {
        let secs = lsc_proof_timeout_secs()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "disabled".to_string());
        return format!(
            "LemmaScript/Dafny proof timed out after {secs}s. Simplify //@ contracts or raise FORALL_LSC_TIMEOUT_SECS / FORALL_PROOF_TIMEOUT_SECS (set 0 to disable)."
        );
    }
    if lsc_install::lsc_is_available() {
        "LemmaScript/Dafny proof obligations failed. Fix //@ contracts and .dfy proofs; run `forall sync` after annotation edits.".to_string()
    } else {
        lsc_install_hint()
    }
}

fn run_lsc(args: &[String], cwd: &Path) -> BackendRunResult {
    let (cmd, prefix_args) = resolve_lsc_command();
    let mut full_args = prefix_args;
    full_args.extend(args.iter().cloned());
    proof_runner::run_command_with_timeout_and_path(
        &cmd,
        &full_args,
        cwd,
        lsc_proof_timeout_secs(),
        "LemmaScript proof",
        // LemmaScript shells out to a bare `dafny`, so installing one under
        // $FORALL_HOME does nothing unless the child can see it. Prepending
        // rather than replacing keeps a user's own Dafny winning if they have
        // one on PATH already.
        super::dafny_install::forall_dafny_bin_dir(),
    )
}

pub fn lsc_error_detail(result: &BackendRunResult) -> String {
    let text = format!("{}{}", result.stderr, result.stdout)
        .trim()
        .to_string();
    if text.is_empty() {
        return format!("command failed: {}", result.command);
    }
    format_diagnostic_output(&text)
}

pub fn lsc_extract(root: &Path, rel_file: &str) -> Option<ExtractResult> {
    let result = run_lsc(&[String::from("extract"), rel_file.to_string()], root);
    if !result.ok {
        return None;
    }
    parse_extract_stdout(&result.stdout)
}

pub fn lsc_check(root: &Path, rel_file: &str) -> BackendRunResult {
    run_lsc(
        &[
            String::from("check"),
            String::from("--backend=dafny"),
            rel_file.to_string(),
        ],
        root,
    )
}

pub fn lsc_regen(root: &Path, rel_file: &str) -> BackendRunResult {
    run_lsc(&[String::from("regen"), rel_file.to_string()], root)
}

fn parse_extract_stdout(stdout: &str) -> Option<ExtractResult> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let functions = parsed
        .get("functions")?
        .as_array()?
        .iter()
        .filter_map(|f| {
            Some(crate::mapping::validate::ExtractedFunction {
                name: f.get("name")?.as_str()?.to_string(),
                requires: f
                    .get("requires")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                ensures: f
                    .get("ensures")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                contract: f
                    .get("contract")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect();
    Some(ExtractResult { functions })
}

pub struct LemmaScriptBackend;

impl ProofBackend for LemmaScriptBackend {
    fn id(&self) -> &'static str {
        PROOF_LEMMASCRIPT_DAFNY
    }

    fn tool_version(&self) -> Option<String> {
        static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        VERSION
            .get_or_init(super::lsc_install::lsc_version_check)
            .clone()
    }

    fn extract_contracts(
        &self,
        root: &Path,
        file: &str,
    ) -> (Option<ExtractResult>, Option<BackendRunResult>) {
        let result = run_lsc(&[String::from("extract"), file.to_string()], root);
        if !result.ok {
            return (None, Some(result));
        }
        match parse_extract_stdout(&result.stdout) {
            Some(extract) => (Some(extract), None),
            None => (None, Some(result)),
        }
    }

    fn run_check(&self, root: &Path, files: &[String]) -> Vec<BackendCheckOutput> {
        super::run_per_file_check("LemmaScript", root, files, lsc_check)
    }

    fn run_sync(&self, root: &Path, file: &str) -> BackendRunResult {
        lsc_regen(root, file)
    }

    fn install_hint(&self) -> String {
        lsc_install_hint()
    }

    fn proof_failure_hint(&self, result: &BackendRunResult) -> String {
        lsc_proof_failure_hint(result)
    }

    fn error_detail(&self, result: &BackendRunResult) -> String {
        lsc_error_detail(result)
    }

    fn doctor_check(&self) -> WorkflowDoctorCheck {
        let (cmd, args) = resolve_lsc_command();
        let mut details = vec![format!("resolved command: {cmd} {}", args.join(" "))];
        if env::var("FORALL_LSC")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            details.push("FORALL_LSC override is set".to_string());
        }

        if cmd == "node" && !args.is_empty() {
            let script = PathBuf::from(&args[0]);
            details.push(format!("lsc script: {}", script.display()));
            if script.is_file() {
                if script.starts_with(lsc_install::forall_lsc_install_root()) {
                    details.push("source: forall ensure-lsc".to_string());
                }
                return WorkflowDoctorCheck::new(
                    "verified.proof_engine.lemmascript",
                    WorkflowDoctorStatus::Ok,
                    "LemmaScript proof engine found",
                )
                .details(details);
            }
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.lemmascript",
                WorkflowDoctorStatus::Fail,
                "LemmaScript proof engine script missing",
            )
            .details(details)
            .remediation(lsc_install_hint());
        }

        if cmd == "npx" {
            details
                .push("fallback uses npx lemmascript (network required on first run)".to_string());
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.lemmascript",
                WorkflowDoctorStatus::Warning,
                "LemmaScript resolves via npx fallback",
            )
            .details(details)
            .remediation(lsc_install_hint());
        }

        if super::path_lookup(&cmd).is_some() {
            details.push(format!("command `{cmd}` found on PATH"));
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.lemmascript",
                WorkflowDoctorStatus::Warning,
                "LemmaScript command found but not validated",
            )
            .details(details)
            .remediation("Prefer a local LemmaScript install or set FORALL_LSC explicitly.");
        }

        WorkflowDoctorCheck::new(
            "verified.proof_engine.lemmascript",
            WorkflowDoctorStatus::Fail,
            "LemmaScript proof engine command not found",
        )
        .details(details)
        .remediation(lsc_install_hint())
    }
}

pub const BACKEND: LemmaScriptBackend = LemmaScriptBackend;

pub fn collect_lemmascript_extracts(
    root: &Path,
    files: &[String],
) -> (
    HashMap<String, ExtractResult>,
    HashMap<String, BackendRunResult>,
) {
    let mut extracts = HashMap::new();
    let mut errors = HashMap::new();
    for file in files {
        let (extract, error) = BACKEND.extract_contracts(root, file);
        match extract {
            Some(extract) => {
                extracts.insert(file.clone(), extract);
            }
            None => {
                if let Some(err) = error {
                    errors.insert(file.clone(), err);
                }
            }
        }
    }
    (extracts, errors)
}
