//! OpenJML proof backend — Java/JML formal verification via `openjml -esc`.
//!
//! OpenJML source tie-in: the `openjml` script in the OpenJML repo runs ESC
//! (Extended Static Checking). JML specs use line comments above methods:
//!
//! ```java
//! //@ requires values != null;
//! //@ ensures -1 < \result;
//! int binarySearch(int[] values, int x) { ... }
//! ```
//!
//! See `OpenJMLTest/test/binarySearch/BS.java` in the OpenJML repo.

use std::env;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::format_diagnostic_output;
use crate::doctor::{WorkflowDoctorCheck, WorkflowDoctorStatus};
use crate::mapping::validate::{ExtractResult, ExtractedFunction};
use crate::verify::adapters::PROOF_OPENJML;

use super::openjml_install;
use super::proof_runner::{self, openjml_proof_timeout_secs};
use super::{BackendCheckOutput, BackendRunResult, ProofBackend};

static JAVA_METHOD_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches method headers like: int binarySearch(int[] values, int x)
    // or: public static int clamp(int x)
    Regex::new(
        r"(?m)^\s*(?:(?:public|private|protected)\s+)?(?:static\s+)?(?:[\w\[\]<>?,]+\s+)+([A-Za-z_][A-Za-z0-9_]*)\s*\(",
    )
    .unwrap_or_else(|err| panic!("invalid Java method header regex: {err}"))
});

/// Parse JML `//@ requires` / `//@ ensures` annotations from Java source.
///
/// Unlike Verus (specs below the `fn` line), JML annotations sit **above** the method.
pub fn extract_jml_contracts(content: &str) -> ExtractResult {
    let mut functions = Vec::new();

    for cap in JAVA_METHOD_PATTERN.captures_iter(content) {
        let Some(name_match) = cap.get(1) else {
            continue;
        };
        let name = name_match.as_str().to_string();
        let method_start = cap.get(0).map(|m| m.start()).unwrap_or(0);

        let (requires, ensures) = jml_annotations_before(content, method_start);
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

/// Collect consecutive `//@ ...` lines immediately above a method.
fn jml_annotations_before(content: &str, method_start: usize) -> (Vec<String>, Vec<String>) {
    let before = &content[..method_start];
    let mut annotation_lines: Vec<String> = Vec::new();

    for line in before.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with("//@") {
            annotation_lines.push(trimmed.to_string());
        } else if trimmed.is_empty() {
            // Allow blank lines between annotations and method.
            continue;
        } else {
            break;
        }
    }

    annotation_lines.reverse();

    let mut requires = Vec::new();
    let mut ensures = Vec::new();
    for line in annotation_lines {
        if line.contains("requires") {
            requires.push(line);
        } else if line.contains("ensures") {
            ensures.push(line);
        }
    }

    (requires, ensures)
}

pub fn openjml_extract(root: &Path, rel_file: &str) -> Option<ExtractResult> {
    let abs = root.join(rel_file);
    let content = fs::read_to_string(abs).ok()?;
    Some(extract_jml_contracts(&content))
}

pub fn openjml_install_hint() -> String {
    openjml_install::openjml_install_hint()
}

pub fn openjml_proof_failure_hint(result: &BackendRunResult) -> String {
    if result.timed_out {
        let secs = openjml_proof_timeout_secs()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "disabled".to_string());
        return format!(
            "OpenJML proof timed out after {secs}s. Simplify JML contracts (scalar methods first) or raise FORALL_OPENJML_TIMEOUT_SECS / FORALL_PROOF_TIMEOUT_SECS (set 0 to disable)."
        );
    }
    if openjml_install::openjml_is_available() {
        "OpenJML proof obligations failed. Start with scalar `//@ requires` / `//@ ensures` on single methods; avoid recursive array sums, \\sum, and heavy loop invariants. Keep array assembly spec-tracked (`verified: false`) until core lemmas pass.".to_string()
    } else {
        openjml_install_hint()
    }
}

fn run_openjml(args: &[String], cwd: &Path) -> BackendRunResult {
    let (cmd, prefix_args) = openjml_install::resolve_openjml_command();
    let mut full_args = prefix_args;
    full_args.extend(args.iter().cloned());
    proof_runner::run_command_with_timeout(
        &cmd,
        &full_args,
        cwd,
        openjml_proof_timeout_secs(),
        "openjml proof",
    )
}

pub fn openjml_error_detail(result: &BackendRunResult) -> String {
    let text = format!("{}{}", result.stderr, result.stdout)
        .trim()
        .to_string();
    if text.is_empty() {
        return format!("command failed: {}", result.command);
    }
    format_diagnostic_output(&text)
}

/// Run ESC static checking on one Java file (OpenJML `-esc` mode).
pub fn openjml_check(root: &Path, rel_file: &str) -> BackendRunResult {
    let mut args = vec![String::from("-esc")];
    if let Some(sourcepath) = openjml_install::java_sourcepath_arg(root) {
        args.push(String::from("-sourcepath"));
        args.push(sourcepath);
    }
    args.push(rel_file.to_string());
    run_openjml(&args, root)
}

pub struct OpenJmlBackend;

impl ProofBackend for OpenJmlBackend {
    fn id(&self) -> &'static str {
        PROOF_OPENJML
    }

    fn tool_version(&self) -> Option<String> {
        static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        VERSION
            .get_or_init(super::openjml_install::openjml_version_check)
            .clone()
    }

    fn extract_contracts(
        &self,
        root: &Path,
        file: &str,
    ) -> (Option<ExtractResult>, Option<BackendRunResult>) {
        match openjml_extract(root, file) {
            Some(extract) => (Some(extract), None),
            None => (
                None,
                Some(BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: format!("could not read or parse {file}"),
                    command: "openjml-extract".to_string(),
                    timed_out: false,
                    tool_version: None,
                }),
            ),
        }
    }

    /// Per-file check — like LemmaScript, not whole-crate like Verus.
    fn run_check(&self, root: &Path, files: &[String]) -> Vec<BackendCheckOutput> {
        super::run_per_file_check("OpenJML", root, files, openjml_check)
    }

    fn run_sync(&self, _root: &Path, _file: &str) -> BackendRunResult {
        BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            command: "openjml-sync-noop".to_string(),
            timed_out: false,
            tool_version: None,
        }
    }

    fn install_hint(&self) -> String {
        openjml_install_hint()
    }

    fn proof_failure_hint(&self, result: &BackendRunResult) -> String {
        openjml_proof_failure_hint(result)
    }

    fn error_detail(&self, result: &BackendRunResult) -> String {
        openjml_error_detail(result)
    }

    fn doctor_check(&self) -> WorkflowDoctorCheck {
        let (cmd, args) = openjml_install::resolve_openjml_command();
        let mut details = vec![format!("resolved command: {cmd} {}", args.join(" "))];
        if env::var("FORALL_OPENJML")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            details.push("FORALL_OPENJML override is set".to_string());
        }

        if openjml_install::openjml_is_available() {
            if let Some(version) = openjml_install::openjml_version_check() {
                details.push(format!("version: {version}"));
            }
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.openjml",
                WorkflowDoctorStatus::Ok,
                "OpenJML proof engine found",
            )
            .details(details);
        }

        WorkflowDoctorCheck::new(
            "verified.proof_engine.openjml",
            WorkflowDoctorStatus::Fail,
            "OpenJML command not found",
        )
        .details(details)
        .remediation(openjml_install_hint())
    }
}

pub const BACKEND: OpenJmlBackend = OpenJmlBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_finds_jml_requires_and_ensures_above_method() {
        let source = r#"
public class BS {
  //@ requires values != null;
  //@ ensures -1 < \result && \result < values.length;
  int binarySearch(int[] values, int x) {
    return 0;
  }
}
"#;
        let extract = extract_jml_contracts(source);
        assert_eq!(extract.functions.len(), 1);
        assert_eq!(extract.functions[0].name, "binarySearch");
        assert!(!extract.functions[0].requires.is_empty());
        assert!(!extract.functions[0].ensures.is_empty());
    }

    #[test]
    fn proof_failure_hint_distinguishes_timeout_from_missing_toolchain() {
        let timed = BackendRunResult {
            ok: false,
            code: None,
            stdout: String::new(),
            stderr: "openjml proof timed out after 180s".to_string(),
            command: "openjml -esc src/Foo.java".to_string(),
            timed_out: true,
            tool_version: None,
        };
        assert!(openjml_proof_failure_hint(&timed).contains("timed out"));

        let failed = BackendRunResult {
            ok: false,
            code: Some(1),
            stdout: "verification failures".to_string(),
            stderr: String::new(),
            command: "openjml -esc src/Foo.java".to_string(),
            timed_out: false,
            tool_version: None,
        };
        let hint = openjml_proof_failure_hint(&failed);
        assert!(
            hint.contains("proof obligations failed") || hint.contains("not found"),
            "hint should guide proof fix or install: {hint}"
        );
    }

    #[test]
    fn extractor_finds_multiple_requires_clauses() {
        let source = r#"
public class BS {
  //@ requires values != null;
  //@ requires (\forall int i, j; 0 <= i && i < j && j < values.length; values[i] <= values[j]);
  //@ ensures -1 < \result && \result < values.length;
  int binarySearch(int[] values, int x) {
    return 0;
  }
}
"#;
        let extract = extract_jml_contracts(source);
        assert_eq!(extract.functions.len(), 1);
        assert_eq!(extract.functions[0].name, "binarySearch");
        assert_eq!(extract.functions[0].requires.len(), 2);
        assert_eq!(extract.functions[0].ensures.len(), 1);
    }
}
