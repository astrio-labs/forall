use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::{Path, PathBuf};

use crate::doctor::WorkflowDoctorCheck;
use crate::mapping::validate::ExtractResult;
use crate::verify::adapters::{
    PROOF_FRAMAC, PROOF_LEMMASCRIPT_DAFNY, PROOF_OPENJML, PROOF_VERUS, VerifiedConfig,
};

pub mod dafny_install;
pub mod framac;
pub mod framac_install;
pub mod lemmascript;
pub mod lsc_install;
pub mod openjml;
pub mod openjml_install;
pub mod proof_runner;
pub mod toolchain_download;
pub mod verus;
pub mod verus_install;

#[derive(Debug, Clone)]
pub struct BackendRunResult {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub command: String,
    pub timed_out: bool,
    /// Version of the prover that produced this result, probed once per
    /// process. Certificates and the obligation ledger need the toolchain
    /// identity behind every verdict; `None` means the probe itself failed.
    pub tool_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BackendCheckOutput {
    pub label: String,
    pub result: BackendRunResult,
}

/// Run a per-file proof check, saying which file is being worked on.
///
/// A proof can take minutes per file, and the check said nothing until every
/// file was done. On a run that hit the timeout there was no way to tell which
/// file was slow, whether anything was still moving, or how far along it was;
/// the whole thing looked hung. Splitting the file to find out is a poor
/// substitute for being told.
///
/// One line before each file so the current one is always named, and one after
/// with the outcome and how long it took. A timeout names the limit, because
/// the next thing a user needs is the knob to raise.
///
/// Progress goes to stderr, so `--json` on stdout is unaffected.
pub fn run_per_file_check(
    tool: &str,
    root: &Path,
    files: &[String],
    check: impl Fn(&Path, &str) -> BackendRunResult,
) -> Vec<BackendCheckOutput> {
    let total = files.len();
    files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let position = format!("[{}/{total}]", index + 1);
            let shown = crate::diagnostic::terminal_safe(file);
            eprintln!("forall check: {tool} {position} {shown}");
            let started = std::time::Instant::now();
            let result = check(root, file);
            let elapsed = started.elapsed().as_secs_f64();
            let outcome = if result.timed_out {
                "timed out"
            } else if result.ok {
                "proved"
            } else {
                "failed"
            };
            eprintln!("forall check: {tool} {position} {shown} — {outcome} in {elapsed:.1}s");
            BackendCheckOutput {
                label: file.clone(),
                result,
            }
        })
        .collect()
}

pub trait ProofBackend {
    fn id(&self) -> &'static str;
    /// The prover's version string, probed at most once per process. `None`
    /// when the tool is missing or the probe fails — callers must not treat
    /// that as "unversioned but trustworthy".
    fn tool_version(&self) -> Option<String>;
    fn extract_contracts(
        &self,
        root: &Path,
        file: &str,
    ) -> (Option<ExtractResult>, Option<BackendRunResult>);
    fn run_check(&self, root: &Path, files: &[String]) -> Vec<BackendCheckOutput>;
    fn run_sync(&self, root: &Path, file: &str) -> BackendRunResult;
    fn install_hint(&self) -> String;
    /// Hint when the toolchain ran but proofs failed (vs missing toolchain).
    fn proof_failure_hint(&self, result: &BackendRunResult) -> String {
        let _ = result;
        self.install_hint()
    }
    fn error_detail(&self, result: &BackendRunResult) -> String;
    fn doctor_check(&self) -> WorkflowDoctorCheck;
}

pub fn backend_for_proof_id(proof_id: &str) -> Option<&'static dyn ProofBackend> {
    match proof_id {
        PROOF_LEMMASCRIPT_DAFNY => Some(&lemmascript::BACKEND),
        PROOF_OPENJML => Some(&openjml::BACKEND),
        PROOF_VERUS => Some(&verus::BACKEND),
        PROOF_FRAMAC => Some(&framac::BACKEND),
        _ => None,
    }
}

pub fn proof_id_for_file(config: &VerifiedConfig, rel_path: &str) -> Option<String> {
    config
        .adapter_for_file(rel_path)
        .filter(|(_, adapter)| adapter.supports_proof())
        .map(|(_, adapter)| adapter.proof.clone())
}

pub fn group_files_by_proof_backend(
    config: &VerifiedConfig,
    files: &[String],
) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        if let Some(proof_id) = proof_id_for_file(config, file) {
            grouped.entry(proof_id).or_default().push(file.clone());
        }
    }
    grouped
}

pub fn collect_extracts(
    root: &Path,
    config: &VerifiedConfig,
    files: &[String],
) -> (
    HashMap<String, ExtractResult>,
    HashMap<String, BackendRunResult>,
) {
    let mut extracts = HashMap::new();
    let mut errors = HashMap::new();
    for file in files {
        let Some(proof_id) = proof_id_for_file(config, file) else {
            continue;
        };
        let Some(backend) = backend_for_proof_id(&proof_id) else {
            continue;
        };
        let (extract, error) = backend.extract_contracts(root, file);
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

pub fn proof_engine_doctor_checks(config: &VerifiedConfig) -> Vec<WorkflowDoctorCheck> {
    let mut checks = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for adapter in config.adapters.values() {
        if !adapter.supports_proof() {
            continue;
        }
        if !seen.insert(adapter.proof.clone()) {
            continue;
        }
        if let Some(backend) = backend_for_proof_id(&adapter.proof) {
            checks.push(backend.doctor_check());
        }
    }
    checks
}

pub(crate) fn path_lookup(command: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        let candidate = dir.join(command);
        if candidate.is_file() {
            Some(candidate)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(ok: bool, timed_out: bool) -> BackendRunResult {
        BackendRunResult {
            ok,
            code: Some(if ok { 0 } else { 1 }),
            stdout: String::new(),
            stderr: String::new(),
            command: "fake".to_string(),
            timed_out,
            tool_version: None,
        }
    }

    /// Progress reporting must not disturb what the check returns: the same
    /// outputs, in the same order, one per file. The proof verdicts are what
    /// the ledger is built from, and a reporting change that reordered or
    /// dropped them would rewrite evidence.
    #[test]
    fn every_file_is_checked_once_and_in_order() {
        let files = vec![
            "src/a.java".to_string(),
            "src/b.java".to_string(),
            "src/c.java".to_string(),
        ];
        let seen = std::sync::Mutex::new(Vec::new());

        let outputs = run_per_file_check("OpenJML", Path::new("/project"), &files, |root, file| {
            assert_eq!(root, Path::new("/project"));
            if let Ok(mut seen) = seen.lock() {
                seen.push(file.to_string());
            }
            match file {
                "src/b.java" => result(false, true),
                "src/c.java" => result(false, false),
                _ => result(true, false),
            }
        });

        assert_eq!(
            seen.into_inner().unwrap_or_default(),
            files,
            "each file checked once, in the order given"
        );
        assert_eq!(
            outputs.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
            ["src/a.java", "src/b.java", "src/c.java"]
        );
        assert!(outputs[0].result.ok);
        assert!(outputs[1].result.timed_out, "the timeout is preserved");
        assert!(!outputs[2].result.ok && !outputs[2].result.timed_out);
    }

    /// A mapping is project-supplied, and `forall check` runs on repositories
    /// that arrived from elsewhere. A path carrying a newline could forge a
    /// second progress line claiming a file was proved when nothing checked
    /// it, and an escape sequence could repaint the terminal around the real
    /// result.
    #[test]
    fn a_crafted_path_cannot_forge_a_progress_line() {
        let forged = crate::diagnostic::terminal_safe("ok.ts\nforall check: OpenJML [9/9] evil.ts — proved in 0.1s");
        assert!(!forged.contains('\n'), "a newline would start a line of its own: {forged}");
        assert!(forged.contains("\\u{000a}"), "{forged}");

        let repainted = crate::diagnostic::terminal_safe("src/\u{1b}[2Ja.ts");
        assert!(!repainted.contains('\u{1b}'), "{repainted}");
        assert!(repainted.contains("\\u{001b}"), "{repainted}");

        // Reorders the rest of the line without being a control character.
        let flipped = crate::diagnostic::terminal_safe("src/\u{202e}sj.ts");
        assert!(!flipped.contains('\u{202e}'), "{flipped}");

        // An ordinary path is left exactly as written.
        assert_eq!(crate::diagnostic::terminal_safe("src/domain/money.ts"), "src/domain/money.ts");
        assert_eq!(crate::diagnostic::terminal_safe("src/café/naïve.ts"), "src/café/naïve.ts");
    }

    /// An empty group must not announce work it is not doing.
    #[test]
    fn no_files_means_no_output() {
        let outputs = run_per_file_check("OpenJML", Path::new("/project"), &[], |_, _| {
            panic!("must not run a check when there are no files")
        });
        assert!(outputs.is_empty());
    }
}
