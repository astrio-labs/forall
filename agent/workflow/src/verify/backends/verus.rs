use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::format_diagnostic_output;
use crate::doctor::{WorkflowDoctorCheck, WorkflowDoctorStatus};
use crate::mapping::validate::{ExtractResult, ExtractedFunction};
use crate::verify::adapters::PROOF_VERUS;

use super::proof_runner::{self, verus_proof_timeout_secs};
use super::verus_install;
use super::{BackendCheckOutput, BackendRunResult, ProofBackend};

static FN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^(?:\s*(?:pub(?:\([^)]*\))?\s+)?(?:proof\s+)?(?:spec\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*))",
    )
    .unwrap_or_else(|err| panic!("invalid Verus fn pattern regex: {err}"))
});

static REQUIRES_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*requires\b")
        .unwrap_or_else(|err| panic!("invalid Verus requires regex: {err}"))
});
static ENSURES_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*ensures\b")
        .unwrap_or_else(|err| panic!("invalid Verus ensures regex: {err}"))
});

pub fn resolve_verus_command() -> (String, Vec<String>) {
    if let Ok(explicit) = env::var("FORALL_VERUS") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some((cmd, rest)) = parts.split_first() {
                return (
                    cmd.to_string(),
                    rest.iter().map(|s| (*s).to_string()).collect(),
                );
            }
        }
    }
    let wrapper = verus_install::forall_cargo_verus_wrapper();
    if wrapper.is_file() {
        return (wrapper.display().to_string(), Vec::new());
    }
    ("cargo".to_string(), vec!["verus".to_string()])
}

pub fn verus_install_hint() -> String {
    verus_install::verus_install_hint()
}

pub fn verus_proof_failure_hint(result: &BackendRunResult) -> String {
    if result.timed_out {
        let secs = verus_proof_timeout_secs()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "disabled".to_string());
        return format!(
            "Verus proof timed out after {secs}s. Simplify requires/ensures/proof blocks or raise FORALL_VERUS_TIMEOUT_SECS / FORALL_PROOF_TIMEOUT_SECS (set 0 to disable)."
        );
    }
    if verus_install::verus_is_available() {
        "Verus proof obligations failed. Fix requires/ensures/proof blocks or run `cargo verus verify` locally for detail.".to_string()
    } else {
        verus_install_hint()
    }
}

fn run_verus(args: &[String], cwd: &Path) -> BackendRunResult {
    let (cmd, prefix_args) = resolve_verus_command();
    let mut full_args = prefix_args;
    full_args.extend(args.iter().cloned());
    proof_runner::run_command_with_timeout(
        &cmd,
        &full_args,
        cwd,
        verus_proof_timeout_secs(),
        "Verus proof",
    )
}

pub fn verus_error_detail(result: &BackendRunResult) -> String {
    let text = format!("{}{}", result.stderr, result.stdout)
        .trim()
        .to_string();
    if text.is_empty() {
        return format!("command failed: {}", result.command);
    }
    format_diagnostic_output(&text)
}

/// Lightweight regex-based extraction of Verus `requires` / `ensures` clauses.
pub fn extract_verus_contracts(content: &str) -> ExtractResult {
    let mut functions = Vec::new();
    for cap in FN_PATTERN.captures_iter(content) {
        let Some(name_match) = cap.get(1) else {
            continue;
        };
        let name = name_match.as_str().to_string();
        let start = cap.get(0).map(|m| m.start()).unwrap_or(0);
        let body_start = find_fn_body_start(content, start);
        let header = &content[start..body_start.min(content.len())];

        let requires = extract_spec_clauses(header, "requires");
        let ensures = extract_spec_clauses(header, "ensures");
        let contract = requires.iter().chain(ensures.iter()).cloned().collect();

        functions.push(ExtractedFunction {
            name,
            requires,
            ensures,
            contract,
        });
    }
    ExtractResult { functions }
}

fn find_fn_body_start(content: &str, fn_start: usize) -> usize {
    let tail = &content[fn_start..];
    if let Some(open) = tail.find('{') {
        return fn_start + open;
    }
    if let Some(semi) = tail.find(';') {
        return fn_start + semi;
    }
    content.len()
}

fn extract_spec_clauses(header: &str, keyword: &str) -> Vec<String> {
    let pattern = if keyword == "requires" {
        &*REQUIRES_PATTERN
    } else {
        &*ENSURES_PATTERN
    };
    let mut clauses = Vec::new();
    for cap in pattern.find_iter(header) {
        let line_start = cap.start();
        let rest = &header[line_start..];
        let clause = rest
            .lines()
            .take_while(|line| {
                let trimmed = line.trim();
                trimmed.is_empty()
                    || trimmed.starts_with(keyword)
                    || trimmed.starts_with(',')
                    || trimmed.ends_with(',')
            })
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .trim_end_matches(',')
            .to_string();
        if !clause.is_empty() {
            clauses.push(clause);
        }
    }
    clauses
}

pub fn verus_extract(root: &Path, rel_file: &str) -> Option<ExtractResult> {
    let abs = root.join(rel_file);
    let content = fs::read_to_string(abs).ok()?;
    Some(extract_verus_contracts(&content))
}

fn find_crate_root(root: &Path, rel_file: &str) -> Option<PathBuf> {
    let file_path = root.join(rel_file);
    let mut dir = file_path.parent()?.to_path_buf();
    loop {
        let cargo = dir.join("Cargo.toml");
        if cargo.is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    let root_cargo = root.join("Cargo.toml");
    if root_cargo.is_file() {
        return Some(root.to_path_buf());
    }
    None
}

fn crate_roots_for_files(root: &Path, files: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for file in files {
        if let Some(crate_root) = find_crate_root(root, file)
            && !roots.iter().any(|existing| existing == &crate_root)
        {
            roots.push(crate_root);
        }
    }
    roots
}

pub fn cargo_verus_metadata_ok(cargo_toml: &Path) -> bool {
    let Ok(content) = fs::read_to_string(cargo_toml) else {
        return false;
    };
    content.contains("[package.metadata.verus]")
        && content.contains("verify")
        && (content.contains("verify = true") || content.contains("verify=true"))
}

const VERUS_CARGO_METADATA: &str = "\n[package.metadata.verus]\nverify = true\n";

/// Append Verus verify metadata to Cargo.toml when missing.
pub fn ensure_cargo_verus_metadata(cargo_toml: &Path) -> std::io::Result<bool> {
    if !cargo_toml.is_file() || cargo_verus_metadata_ok(cargo_toml) {
        return Ok(false);
    }
    let mut content = fs::read_to_string(cargo_toml)?;
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(VERUS_CARGO_METADATA);
    fs::write(cargo_toml, content)?;
    Ok(true)
}

pub fn find_cargo_toml_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_cargo_toml_files(root, &mut files, 0);
    files
}

fn collect_cargo_toml_files(dir: &Path, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if depth > 0
        && matches!(
            name,
            "target" | "node_modules" | ".git" | ".forall" | "vendor" | "dist" | "build"
        )
    {
        return;
    }
    let cargo = dir.join("Cargo.toml");
    if cargo.is_file() {
        files.push(cargo);
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_cargo_toml_files(&path, files, depth + 1);
        }
    }
}

pub fn project_has_rust_sources(root: &Path) -> bool {
    !collect_rust_source_files(root, root, 0).is_empty()
}

pub fn collect_rust_source_files(root: &Path, dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_rust_source_files_inner(root, dir, &mut files, depth);
    files
}

fn collect_rust_source_files_inner(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if depth > 0
        && matches!(
            name,
            "target" | "node_modules" | ".git" | ".forall" | "vendor" | "dist" | "build"
        )
    {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_source_files_inner(root, &path, files, depth + 1);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
}

/// Whether Verus can reach a Z3.
///
/// Every Verus release zip ships a `z3` beside the launcher, and the launcher
/// resolves it itself — `source/verus/src/main.rs` sets `VERUS_Z3_PATH` from
/// its own parent directory when that file exists. So the bundled copy is
/// found without ever being on PATH.
///
/// This used to be a bare `Command::new("z3")` PATH probe, which fails on a
/// correctly installed machine. The remediation it triggered — "Install Z3 and
/// ensure `z3` is on PATH" — sent users to install a second Z3 they already
/// had and did not need, and to put it somewhere Verus does not look.
/// It has to be the Z3 belonging to the Verus that will run. Scanning every
/// release directory for any `z3` reported success for a solver the selected
/// Verus never consults — a stale release still on disk, or a managed release
/// while `FORALL_VERUS` points elsewhere — so `forall doctor` said OK and the
/// proof then failed for want of a solver.
pub fn z3_available() -> bool {
    if let Some(binary) = active_verus_binary() {
        // The launcher prefers the copy beside itself, so that one settles it.
        if z3_beside(&binary).is_some() {
            return true;
        }
    }
    // An explicit override is what Verus honours next, whichever binary runs.
    if let Ok(configured) = std::env::var("VERUS_Z3_PATH")
        && Path::new(&configured).is_file()
    {
        return true;
    }
    Command::new("z3")
        .arg("-version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// The `verus` binary the active command will run, when that can be known.
///
/// `FORALL_VERUS` and a bare `cargo verus` from PATH both name programs Forall
/// did not install and cannot locate, so both answer `None`: the solver beside
/// a managed release says nothing about either, and claiming otherwise is what
/// produced the false OK. The managed wrapper names its Verus outright, which
/// is why it is read rather than inferred from the directory layout — the
/// wrapper is what actually runs.
fn active_verus_binary() -> Option<PathBuf> {
    if env::var("FORALL_VERUS").is_ok_and(|value| !value.trim().is_empty()) {
        return None;
    }
    let wrapper = verus_install::forall_cargo_verus_wrapper();
    let script = std::fs::read_to_string(&wrapper).ok()?;
    script
        .lines()
        .find_map(|line| line.trim().strip_prefix("VERUS_BIN="))
        .map(|value| PathBuf::from(value.trim().trim_matches('"')))
}

/// The `z3` a Verus launcher resolves for itself.
///
/// Every release zip ships one beside the launcher, and
/// `source/verus/src/main.rs` sets `VERUS_Z3_PATH` from its own parent
/// directory when that file exists, so the bundled copy is reached without
/// ever being on PATH.
fn z3_beside(verus_binary: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "z3.exe" } else { "z3" };
    let candidate = verus_binary.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

pub struct VerusBackend;

impl ProofBackend for VerusBackend {
    fn id(&self) -> &'static str {
        PROOF_VERUS
    }

    fn tool_version(&self) -> Option<String> {
        static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        VERSION
            .get_or_init(super::verus_install::verus_version_check)
            .clone()
    }

    fn extract_contracts(
        &self,
        root: &Path,
        file: &str,
    ) -> (Option<ExtractResult>, Option<BackendRunResult>) {
        match verus_extract(root, file) {
            Some(extract) => (Some(extract), None),
            None => (
                None,
                Some(BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: format!("could not read or parse {file}"),
                    command: "verus-extract".to_string(),
                    timed_out: false,
                    tool_version: None,
                }),
            ),
        }
    }

    fn run_check(&self, root: &Path, files: &[String]) -> Vec<BackendCheckOutput> {
        let crate_roots = crate_roots_for_files(root, files);
        if crate_roots.is_empty() {
            return vec![BackendCheckOutput {
                label: "Cargo.toml".to_string(),
                result: BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: "no Cargo.toml found for verified .rs files".to_string(),
                    command: "cargo verus verify".to_string(),
                    timed_out: false,
                    tool_version: None,
                },
            }];
        }

        crate_roots
            .into_iter()
            .map(|crate_root| {
                let label = crate_root
                    .strip_prefix(root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| crate_root.display().to_string());
                let result = run_verus(&[String::from("verify")], &crate_root);
                BackendCheckOutput { label, result }
            })
            .collect()
    }

    fn run_sync(&self, _root: &Path, _file: &str) -> BackendRunResult {
        BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            command: "verus-sync-noop".to_string(),
            timed_out: false,
            tool_version: None,
        }
    }

    fn install_hint(&self) -> String {
        verus_install_hint()
    }

    fn proof_failure_hint(&self, result: &BackendRunResult) -> String {
        verus_proof_failure_hint(result)
    }

    fn error_detail(&self, result: &BackendRunResult) -> String {
        verus_error_detail(result)
    }

    fn doctor_check(&self) -> WorkflowDoctorCheck {
        let (cmd, args) = resolve_verus_command();
        let mut details = vec![format!("resolved command: {cmd} {}", args.join(" "))];
        if env::var("FORALL_VERUS")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            details.push("FORALL_VERUS override is set".to_string());
        }

        if !Path::new(&cmd).is_file() && super::path_lookup(&cmd).is_none() && cmd != "cargo" {
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.verus",
                WorkflowDoctorStatus::Fail,
                "Verus command not found",
            )
            .details(details)
            .remediation(verus_install::verus_install_hint());
        }

        let version = Command::new(&cmd).args(&args).arg("--version").output();
        match version {
            Ok(out) if out.status.success() => {
                let version_text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !version_text.is_empty() {
                    details.push(format!("version: {version_text}"));
                }
            }
            Ok(out) => {
                details.push(format!(
                    "version check failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            Err(err) => {
                return WorkflowDoctorCheck::new(
                    "verified.proof_engine.verus",
                    WorkflowDoctorStatus::Fail,
                    "Verus command not found",
                )
                .details(details)
                .detail(err.to_string())
                .remediation(verus_install::verus_install_hint());
            }
        }

        if z3_available() {
            details.push("z3: available".to_string());
        } else {
            details.push("z3: not bundled with this Verus and not on PATH".to_string());
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.verus",
                WorkflowDoctorStatus::Warning,
                "Verus found but no Z3 solver reachable",
            )
            .details(details)
            .remediation(
                "Official Verus releases ship a z3 beside the binary; re-run `forall ensure-verus` to install one, or set VERUS_Z3_PATH to an existing z3.",
            );
        }

        WorkflowDoctorCheck::new(
            "verified.proof_engine.verus",
            WorkflowDoctorStatus::Ok,
            "Verus proof engine found",
        )
        .details(details)
    }
}

pub const BACKEND: VerusBackend = VerusBackend;

pub fn doctor_cargo_metadata_check(root: &Path) -> Option<WorkflowDoctorCheck> {
    let cargo = root.join("Cargo.toml");
    if !cargo.is_file() {
        return None;
    }
    let has_rs = walkdir_rs_files(root);
    if has_rs.is_empty() {
        return None;
    }

    let mut details = vec![format!("Cargo.toml: {}", cargo.display())];
    if cargo_verus_metadata_ok(&cargo) {
        details.push("[package.metadata.verus] verify = true: present".to_string());
        return Some(
            WorkflowDoctorCheck::new(
                "verified.verus_cargo",
                WorkflowDoctorStatus::Ok,
                "Cargo.toml has Verus verify metadata",
            )
            .details(details),
        );
    }

    details.push("[package.metadata.verus] verify = true: missing".to_string());
    Some(
        WorkflowDoctorCheck::new(
            "verified.verus_cargo",
            WorkflowDoctorStatus::Warning,
            "Rust project missing Verus Cargo metadata",
        )
        .details(details)
        .remediation("Add to Cargo.toml:\n[package.metadata.verus]\nverify = true"),
    )
}

fn walkdir_rs_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let src = root.join("src");
    if !src.is_dir() {
        return files;
    }
    if let Ok(entries) = fs::read_dir(&src) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path.display().to_string());
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_requires_and_ensures() {
        let source = r#"
pub fn clamp(x: u64, lo: u64, hi: u64) -> (result: u64)
    requires lo <= hi,
    ensures lo <= result <= hi,
{
    if x < lo { lo } else if x > hi { hi } else { x }
}
"#;
        let extract = extract_verus_contracts(source);
        assert_eq!(extract.functions.len(), 1);
        assert_eq!(extract.functions[0].name, "clamp");
        assert!(!extract.functions[0].requires.is_empty());
        assert!(!extract.functions[0].ensures.is_empty());
    }

    #[test]
    fn detects_cargo_verus_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let cargo = dir.path().join("Cargo.toml");
        fs::write(
            &cargo,
            "[package]\nname = \"demo\"\n\n[package.metadata.verus]\nverify = true\n",
        )
        .unwrap();
        assert!(cargo_verus_metadata_ok(&cargo));
    }

    /// The solver has to be the one belonging to the Verus that will run.
    /// A `z3` under some other release directory is not reachable by the
    /// active launcher, and reporting it as available made `forall doctor`
    /// say OK while the proof failed for want of a solver.
    #[test]
    fn a_z3_under_another_release_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        let active = dir.path().join("active");
        let stale = dir.path().join("stale");
        fs::create_dir_all(&active).unwrap();
        fs::create_dir_all(&stale).unwrap();

        let active_verus = active.join("verus");
        fs::write(&active_verus, "binary").unwrap();
        // Only the stale release has a solver beside it.
        fs::write(stale.join("z3"), "binary").unwrap();

        assert_eq!(
            z3_beside(&active_verus),
            None,
            "the active launcher cannot reach a z3 in another directory"
        );

        let expected = active.join("z3");
        fs::write(&expected, "binary").unwrap();
        assert_eq!(
            z3_beside(&active_verus),
            Some(expected),
            "the launcher resolves the copy beside itself"
        );
    }

    /// The wrapper names the Verus it runs, so that name decides which z3 is
    /// consulted. Reading it keeps the probe pointed at the same binary the
    /// proof will use.
    #[test]
    fn the_wrapper_names_the_verus_that_will_run() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("cargo-verus");
        fs::write(
            &wrapper,
            "#!/usr/bin/env bash\nset -euo pipefail\nVERUS_BIN=\"/opt/verus/verus\"\nexec \"$VERUS_BIN\" \"$@\"\n",
        )
        .unwrap();

        let named = fs::read_to_string(&wrapper)
            .unwrap()
            .lines()
            .find_map(|line| line.trim().strip_prefix("VERUS_BIN="))
            .map(|value| PathBuf::from(value.trim().trim_matches('"')));
        assert_eq!(named, Some(PathBuf::from("/opt/verus/verus")));
    }
}
