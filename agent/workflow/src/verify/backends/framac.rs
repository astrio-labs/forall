//! Frama-C proof backend — C/ACSL formal verification via `frama-c -wp`.
//!
//! ACSL contracts sit **above** the function, as `//@` line comments or a
//! `/*@ ... */` block:
//!
//! ```c
//! /*@ requires n > 0;
//!     requires \valid_read(a + (0 .. n-1));
//!     ensures \forall integer i; 0 <= i < n ==> \result >= a[i];
//! */
//! int max_elem(const int *a, int n) { ... }
//! ```
//!
//! WP discharges proof obligations through SMT solvers (Alt-Ergo by default).
//! One trap this module owns: `frama-c -wp` exits 0 even when goals are left
//! unproved — success means "the analysis ran". `enforce_wp_proved_goals`
//! parses the `Proved goals: X / Y` summary and fails the run when X < Y.

use std::env;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::diagnostic::format_diagnostic_output;
use crate::doctor::{WorkflowDoctorCheck, WorkflowDoctorStatus};
use crate::mapping::validate::{ExtractResult, ExtractedFunction};
use crate::verify::adapters::PROOF_FRAMAC;

use super::framac_install;
use super::proof_runner::{self, framac_proof_timeout_secs};
use super::{BackendCheckOutput, BackendRunResult, ProofBackend};

static C_FUNCTION_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches function headers like: int max_elem(const int *a, int n)
    // or: static unsigned long *lookup(struct table *t)
    Regex::new(r"(?m)^\s*(?:[\w\*]+\s+)+\*?\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .unwrap_or_else(|err| panic!("invalid C function header regex: {err}"))
});

static C_FN_PTR_RETURN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Function-pointer-returning declarators: int (*get_handler(void))(int)
    Regex::new(r"(?m)^\s*(?:[\w\*]+\s+)+\(\s*\*\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .unwrap_or_else(|err| panic!("invalid C function-pointer return regex: {err}"))
});

static C_ATTRIBUTE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // GNU __attribute__((...)) (one nesting level, e.g. format(printf,1,2)),
    // MSVC __declspec(...), and C23 [[...]] attributes.
    Regex::new(
        r"__attribute__\s*\(\((?:[^()]|\([^()]*\))*\)\)|__declspec\s*\([^)]*\)|\[\[[^\]]*\]\]",
    )
    .unwrap_or_else(|err| panic!("invalid C attribute regex: {err}"))
});

/// Blank out declaration attributes with equal-length spaces so the function
/// regexes see plain headers while every byte offset (and line structure)
/// stays valid for the annotation scan.
fn blank_attributes(content: &str) -> String {
    C_ATTRIBUTE_PATTERN
        .replace_all(content, |cap: &regex::Captures<'_>| {
            " ".repeat(cap[0].len())
        })
        .into_owned()
}

static PROVED_GOALS_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // WP summary line, e.g. "[wp] Proved goals:   12 / 14"
    Regex::new(r"Proved goals:\s*(\d+)\s*/\s*(\d+)")
        .unwrap_or_else(|err| panic!("invalid WP proved-goals regex: {err}"))
});

/// C keywords that the function-header regex can capture as a "name".
fn is_c_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "while" | "for" | "switch" | "return" | "sizeof" | "else" | "do" | "case" | "goto"
    )
}

/// Parse ACSL `requires` / `ensures` annotations from C source.
///
/// Like JML (and unlike Verus), ACSL annotations sit **above** the function.
pub fn extract_acsl_contracts(content: &str) -> ExtractResult {
    // Attributes are blanked (not removed) so offsets into `scrubbed` are
    // also valid offsets into the original content.
    let scrubbed = blank_attributes(content);
    let mut headers: Vec<(usize, String, &str)> = Vec::new();

    for pattern in [&*C_FUNCTION_PATTERN, &*C_FN_PTR_RETURN_PATTERN] {
        for cap in pattern.captures_iter(&scrubbed) {
            let Some(name_match) = cap.get(1) else {
                continue;
            };
            let name = name_match.as_str().to_string();
            if is_c_keyword(&name) {
                continue;
            }
            let whole = cap.get(0).map(|m| m.as_str()).unwrap_or("");
            if whole.trim_start().starts_with("return ") {
                continue;
            }
            let function_start = cap.get(0).map(|m| m.start()).unwrap_or(0);
            headers.push((function_start, name, whole));
        }
    }

    headers.sort_by_key(|(start, _, _)| *start);
    let mut functions = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (function_start, name, _) in headers {
        if !seen.insert(name.clone()) {
            continue;
        }
        let (requires, ensures) = acsl_annotations_before(&scrubbed, function_start);
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

/// Collect the ACSL annotation region immediately above a function: trailing
/// consecutive `//@` lines and/or one `/*@ ... */` block, blank lines allowed.
fn acsl_annotations_before(content: &str, function_start: usize) -> (Vec<String>, Vec<String>) {
    let before = &content[..function_start];
    let lines: Vec<&str> = before.lines().collect();
    let mut collected: Vec<String> = Vec::new(); // reverse order
    let mut i = lines.len();

    while i > 0 {
        let trimmed = lines[i - 1].trim();
        if trimmed.is_empty() {
            i -= 1;
            continue;
        }
        if trimmed.starts_with("//@") {
            collected.push(trimmed.to_string());
            i -= 1;
            continue;
        }
        if trimmed.ends_with("*/") {
            // Walk up to the block opener; keep the block only if it is ACSL (`/*@`).
            let mut block: Vec<String> = Vec::new(); // reverse order
            let mut j = i;
            let mut opened = false;
            while j > 0 {
                let block_line = lines[j - 1].trim();
                block.push(block_line.to_string());
                if block_line.contains("/*") {
                    opened = block_line.contains("/*@");
                    break;
                }
                j -= 1;
            }
            if opened {
                collected.extend(block);
                i = j - 1;
                continue;
            }
        }
        break;
    }

    collected.reverse();
    parse_acsl_clauses(&collected)
}

/// Split collected annotation lines into `requires` / `ensures` clause strings.
fn parse_acsl_clauses(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let cleaned: Vec<String> = lines
        .iter()
        .map(|line| {
            let mut text = line.trim().to_string();
            for marker in ["/*@", "//@", "@*/", "*/"] {
                text = text.replace(marker, " ");
            }
            let text = text.trim();
            text.strip_prefix('@').unwrap_or(text).trim().to_string()
        })
        .collect();

    let mut requires = Vec::new();
    let mut ensures = Vec::new();
    for clause in cleaned.join(" ").split(';') {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }
        if clause.starts_with("requires") {
            requires.push(format!("{clause};"));
        } else if clause.starts_with("ensures") {
            ensures.push(format!("{clause};"));
        }
    }
    (requires, ensures)
}

pub fn framac_extract(root: &Path, rel_file: &str) -> Option<ExtractResult> {
    let abs = root.join(rel_file);
    let content = fs::read_to_string(abs).ok()?;
    Some(extract_acsl_contracts(&content))
}

pub fn framac_install_hint() -> String {
    framac_install::framac_install_hint()
}

pub fn framac_proof_failure_hint(result: &BackendRunResult) -> String {
    if result.timed_out {
        let secs = framac_proof_timeout_secs()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "disabled".to_string());
        return format!(
            "Frama-C proof timed out after {secs}s. Simplify ACSL contracts (scalar functions first) or raise FORALL_FRAMAC_TIMEOUT_SECS / FORALL_PROOF_TIMEOUT_SECS (set 0 to disable)."
        );
    }
    // A fresh Frama-C install aborts until Why3 has registered the provers.
    let combined = format!("{}{}", result.stderr, result.stdout);
    if combined.contains("not found in why3.conf") {
        return "WP cannot find its SMT prover: run `why3 config detect` to register installed provers (e.g. Alt-Ergo), then re-run the check.".to_string();
    }
    // Smoke-test failure means the contract is vacuous, not that the prover is
    // too weak — a fundamentally different fix than strengthening proofs.
    if combined.contains("Failed smoke-test") {
        return "WP smoke test failed: the `requires` clauses are contradictory (unsatisfiable), so every postcondition would be vacuously provable. Fix the preconditions rather than the proof — check for impossible conjunctions like `x > 0 && x < 0` or requires that contradict the function's domain.".to_string();
    }
    if framac_install::framac_is_available() {
        "WP proof obligations failed or were left unproved. Start with scalar `requires` / `ensures` contracts; add `assigns` clauses on functions and `loop invariant` / `loop assigns` / `loop variant` on every loop in verified functions. Avoid casts and pointer arithmetic beyond `\\valid` array indexing (Typed memory model). Keep hard functions spec-tracked (`verified: false`) until core obligations pass.".to_string()
    } else {
        framac_install_hint()
    }
}

fn run_framac(args: &[String], cwd: &Path) -> BackendRunResult {
    let (cmd, prefix_args) = framac_install::resolve_framac_command();
    let mut full_args = prefix_args;
    full_args.extend(args.iter().cloned());
    proof_runner::run_command_with_timeout(
        &cmd,
        &full_args,
        cwd,
        framac_proof_timeout_secs(),
        "frama-c proof",
    )
}

pub fn framac_error_detail(result: &BackendRunResult) -> String {
    let text = format!("{}{}", result.stderr, result.stdout)
        .trim()
        .to_string();
    if text.is_empty() {
        return format!("command failed: {}", result.command);
    }
    format_diagnostic_output(&text)
}

/// Fail runs where WP left goals unproved despite a zero exit code, and fail
/// closed when no `Proved goals: X / Y` summary is present at all — a
/// successful exit without the summary means WP's completeness was never
/// observed (format drift, plugin not loaded, output redirection), and a
/// verifier must not treat that as proof. WP prints the summary even for
/// zero-goal files, so this cannot false-positive on unannotated code.
fn enforce_wp_proved_goals(result: &mut BackendRunResult) {
    if !result.ok || result.timed_out {
        return;
    }
    let mut summary_seen = false;
    for cap in PROVED_GOALS_PATTERN.captures_iter(&result.stdout) {
        summary_seen = true;
        let proved: u64 = cap[1].parse().unwrap_or(0);
        let total: u64 = cap[2].parse().unwrap_or(0);
        if proved < total {
            result.ok = false;
            if !result.stderr.is_empty() {
                result.stderr.push('\n');
            }
            result
                .stderr
                .push_str(&format!("frama-c wp: only {proved}/{total} goals proved"));
            return;
        }
    }
    if !summary_seen {
        result.ok = false;
        if !result.stderr.is_empty() {
            result.stderr.push('\n');
        }
        result.stderr.push_str(
            "frama-c wp: no 'Proved goals' summary in output — cannot confirm proof completeness",
        );
    }
}

/// Run WP (with RTE guards and smoke tests) on one C file.
///
/// `-wp-smoke-tests` guards against vacuous proofs: contradictory `requires`
/// make every postcondition trivially provable, and the smoke goal fails in
/// exactly that case. A failed smoke test is counted as an unproved goal in
/// the `Proved goals: X / Y` summary, so `enforce_wp_proved_goals` catches it.
pub fn framac_check(root: &Path, rel_file: &str) -> BackendRunResult {
    let mut args = vec![
        String::from("-wp"),
        String::from("-wp-rte"),
        String::from("-wp-smoke-tests"),
    ];
    if let Some(db) = framac_install::compile_commands_arg(root) {
        args.push(String::from("-json-compilation-database"));
        args.push(db);
    } else if let Some(cpp) = framac_install::cpp_include_arg(root) {
        args.push(cpp);
    }
    args.push(rel_file.to_string());
    let mut result = run_framac(&args, root);
    enforce_wp_proved_goals(&mut result);
    result
}

pub struct FramacBackend;

impl ProofBackend for FramacBackend {
    fn id(&self) -> &'static str {
        PROOF_FRAMAC
    }

    fn tool_version(&self) -> Option<String> {
        static VERSION: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        VERSION
            .get_or_init(super::framac_install::framac_version_check)
            .clone()
    }

    fn extract_contracts(
        &self,
        root: &Path,
        file: &str,
    ) -> (Option<ExtractResult>, Option<BackendRunResult>) {
        match framac_extract(root, file) {
            Some(extract) => (Some(extract), None),
            None => (
                None,
                Some(BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: format!("could not read or parse {file}"),
                    command: "framac-extract".to_string(),
                    timed_out: false,
                    tool_version: None,
                }),
            ),
        }
    }

    /// Per-file check — like LemmaScript and OpenJML, not whole-crate like Verus.
    fn run_check(&self, root: &Path, files: &[String]) -> Vec<BackendCheckOutput> {
        super::run_per_file_check("Frama-C", root, files, framac_check)
    }

    fn run_sync(&self, _root: &Path, _file: &str) -> BackendRunResult {
        BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            command: "framac-sync-noop".to_string(),
            timed_out: false,
            tool_version: None,
        }
    }

    fn install_hint(&self) -> String {
        framac_install_hint()
    }

    fn proof_failure_hint(&self, result: &BackendRunResult) -> String {
        framac_proof_failure_hint(result)
    }

    fn error_detail(&self, result: &BackendRunResult) -> String {
        framac_error_detail(result)
    }

    fn doctor_check(&self) -> WorkflowDoctorCheck {
        let (cmd, args) = framac_install::resolve_framac_command();
        let mut details = vec![format!("resolved command: {cmd} {}", args.join(" "))];
        if env::var("FORALL_FRAMAC")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
        {
            details.push("FORALL_FRAMAC override is set".to_string());
        }

        if framac_install::framac_is_available() {
            if let Some(version) = framac_install::framac_version_check() {
                details.push(format!("version: {version}"));
            }
            // An explicit `-wp-prover` in FORALL_FRAMAC prefix args means the
            // deployment chose its own prover (e.g. Z3) — skip the Alt-Ergo probe.
            let prover_pinned = args
                .iter()
                .any(|a| a == "-wp-prover" || a.starts_with("-wp-prover="));
            if !prover_pinned && super::path_lookup("alt-ergo").is_none() {
                return WorkflowDoctorCheck::new(
                    "verified.proof_engine.framac",
                    WorkflowDoctorStatus::Warning,
                    "Frama-C found but Alt-Ergo prover missing",
                )
                .details(details)
                .remediation(
                    "Install Alt-Ergo (e.g. `opam install alt-ergo`) and run `why3 config detect` so WP can discharge goals, or configure another prover and pass it via `-wp-prover`.".to_string(),
                );
            }
            return WorkflowDoctorCheck::new(
                "verified.proof_engine.framac",
                WorkflowDoctorStatus::Ok,
                "Frama-C proof engine found",
            )
            .details(details);
        }

        WorkflowDoctorCheck::new(
            "verified.proof_engine.framac",
            WorkflowDoctorStatus::Fail,
            "Frama-C command not found",
        )
        .details(details)
        .remediation(framac_install_hint())
    }
}

pub const BACKEND: FramacBackend = FramacBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_finds_acsl_line_comments_above_function() {
        let source = r#"
//@ requires n > 0;
//@ ensures \result >= 0;
int clamp(int n) {
  return n;
}
"#;
        let extract = extract_acsl_contracts(source);
        assert_eq!(extract.functions.len(), 1);
        assert_eq!(extract.functions[0].name, "clamp");
        assert_eq!(extract.functions[0].requires, vec!["requires n > 0;"]);
        assert_eq!(extract.functions[0].ensures, vec![r"ensures \result >= 0;"]);
    }

    #[test]
    fn extractor_finds_acsl_block_comment_above_function() {
        let source = r#"
#include <stddef.h>

/*@ requires n > 0;
    requires \valid_read(a + (0 .. n-1));
    ensures \forall integer i; 0 <= i < n ==> \result >= a[i];
*/
int max_elem(const int *a, int n) {
  return a[0];
}
"#;
        let extract = extract_acsl_contracts(source);
        let max_elem = extract
            .functions
            .iter()
            .find(|f| f.name == "max_elem")
            .expect("max_elem extracted");
        assert_eq!(max_elem.requires.len(), 2);
        assert_eq!(max_elem.ensures.len(), 1);
        assert!(max_elem.requires[1].contains(r"\valid_read"));
    }

    #[test]
    fn extractor_skips_plain_block_comments_and_keywords() {
        let source = r#"
/* Regular doc comment: ensures nothing formally. */
static unsigned long *lookup(struct table *t) {
  while (t->next) {
    t = advance(t);
  }
  return item_for(t);
}
"#;
        let extract = extract_acsl_contracts(source);
        let names: Vec<&str> = extract.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"lookup"));
        assert!(!names.contains(&"while"));
        assert!(!names.contains(&"return"));
        let lookup = extract
            .functions
            .iter()
            .find(|f| f.name == "lookup")
            .unwrap();
        assert!(lookup.requires.is_empty());
        assert!(lookup.ensures.is_empty());
    }

    #[test]
    fn extractor_handles_attributes_and_fn_ptr_returns() {
        let source = r#"
//@ requires n > 0;
//@ ensures \result > 0;
__attribute__((always_inline)) static int attributed(int n) {
  return n;
}

__attribute__((format(printf, 1, 2))) int log_fmt(const char *fmt, ...) {
  return 0;
}

//@ ensures \result != \null;
int (*get_handler(void))(int) {
  return 0;
}

[[nodiscard]] int c23_attr(int n) {
  return n;
}
"#;
        let extract = extract_acsl_contracts(source);
        let names: Vec<&str> = extract.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"attributed"), "gnu attribute: {names:?}");
        assert!(
            names.contains(&"log_fmt"),
            "nested-paren attribute: {names:?}"
        );
        assert!(names.contains(&"get_handler"), "fn-ptr return: {names:?}");
        assert!(names.contains(&"c23_attr"), "c23 attribute: {names:?}");

        let attributed = extract
            .functions
            .iter()
            .find(|f| f.name == "attributed")
            .unwrap();
        assert_eq!(attributed.requires, vec!["requires n > 0;"]);
        let handler = extract
            .functions
            .iter()
            .find(|f| f.name == "get_handler")
            .unwrap();
        assert_eq!(handler.ensures.len(), 1);
    }

    #[test]
    fn missing_wp_summary_fails_closed() {
        let mut result = BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: "[kernel] Parsing max.c (with preprocessing)\n".to_string(),
            stderr: String::new(),
            command: "frama-c -wp -wp-rte -wp-smoke-tests max.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        enforce_wp_proved_goals(&mut result);
        assert!(
            !result.ok,
            "success without a goal summary must fail closed"
        );
        assert!(result.stderr.contains("cannot confirm proof completeness"));
    }

    #[test]
    fn unproved_goals_fail_despite_zero_exit() {
        let mut result = BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: "[wp] Proved goals:   12 / 14\n".to_string(),
            stderr: String::new(),
            command: "frama-c -wp -wp-rte src/max.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        enforce_wp_proved_goals(&mut result);
        assert!(!result.ok);
        assert!(result.stderr.contains("only 12/14 goals proved"));

        let mut all_proved = BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: "[wp] Proved goals:   14 / 14\n".to_string(),
            stderr: String::new(),
            command: "frama-c -wp -wp-rte src/max.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        enforce_wp_proved_goals(&mut all_proved);
        assert!(all_proved.ok);
    }

    #[test]
    fn smoke_test_failure_counts_as_unproved_and_hints_contradiction() {
        // Real frama-c 32.1 output shape for `requires x > 0 && x < 0;`.
        let stdout = "[wp] [Failed] (Doomed) typed_impossible_wp_smoke_default_requires (Qed)\n\
[wp] vacuous.c:5: Warning: Failed smoke-test\n\
[wp] Proved goals:    4 / 5\n  Smoke Tests:     0 / 1\n";
        let mut result = BackendRunResult {
            ok: true,
            code: Some(0),
            stdout: stdout.to_string(),
            stderr: String::new(),
            command: "frama-c -wp -wp-rte -wp-smoke-tests vacuous.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        enforce_wp_proved_goals(&mut result);
        assert!(!result.ok, "failed smoke test must fail the check");

        let hint = framac_proof_failure_hint(&result);
        assert!(
            hint.contains("contradictory"),
            "smoke failure should hint at contradictory requires, got: {hint}"
        );
    }

    #[test]
    fn proof_failure_hint_detects_unconfigured_why3() {
        let unconfigured = BackendRunResult {
            ok: false,
            code: Some(1),
            stdout: String::new(),
            stderr: "[wp] User Error: Prover 'Alt-Ergo' not found in why3.conf".to_string(),
            command: "frama-c -wp -wp-rte src/max.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        assert!(framac_proof_failure_hint(&unconfigured).contains("why3 config detect"));
    }

    /// Live end-to-end WP run — skips when frama-c or alt-ergo is absent
    /// (same pattern as the node-present property tests).
    #[test]
    fn framac_check_live_when_toolchain_present() {
        if !framac_install::framac_is_available()
            || crate::verify::backends::path_lookup("alt-ergo").is_none()
        {
            eprintln!("skipping framac live test: frama-c or alt-ergo not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        std::fs::write(
            root.join("max.c"),
            r#"/*@ requires a >= 0 && b >= 0;
    ensures \result >= a && \result >= b;
    ensures \result == a || \result == b;
    assigns \nothing;
*/
int max2(int a, int b) {
  return a > b ? a : b;
}
"#,
        )
        .unwrap();
        let ok_result = framac_check(root, "max.c");
        assert!(
            ok_result.ok,
            "provable contract should pass\nstdout: {}\nstderr: {}",
            ok_result.stdout, ok_result.stderr
        );

        std::fs::write(
            root.join("bad.c"),
            r#"/*@ ensures \result == 0;
    assigns \nothing;
*/
int ident(int a) {
  return a;
}
"#,
        )
        .unwrap();
        let bad_result = framac_check(root, "bad.c");
        assert!(
            !bad_result.ok,
            "unprovable contract must fail\nstdout: {}",
            bad_result.stdout
        );

        std::fs::write(
            root.join("vacuous.c"),
            r#"/*@ requires x > 0 && x < 0;
    ensures \result == 42;
    assigns \nothing;
*/
int impossible(int x) {
  return 7;
}
"#,
        )
        .unwrap();
        let vacuous_result = framac_check(root, "vacuous.c");
        assert!(
            !vacuous_result.ok,
            "contradictory requires must fail via smoke test\nstdout: {}",
            vacuous_result.stdout
        );
        assert!(
            framac_proof_failure_hint(&vacuous_result).contains("contradictory"),
            "vacuous contract should get the contradiction hint"
        );
    }

    #[test]
    fn proof_failure_hint_distinguishes_timeout_from_missing_toolchain() {
        let timed = BackendRunResult {
            ok: false,
            code: None,
            stdout: String::new(),
            stderr: "frama-c proof timed out after 300s".to_string(),
            command: "frama-c -wp src/max.c".to_string(),
            timed_out: true,
            tool_version: None,
        };
        assert!(framac_proof_failure_hint(&timed).contains("timed out"));

        let failed = BackendRunResult {
            ok: false,
            code: Some(1),
            stdout: "verification failures".to_string(),
            stderr: String::new(),
            command: "frama-c -wp src/max.c".to_string(),
            timed_out: false,
            tool_version: None,
        };
        let hint = framac_proof_failure_hint(&failed);
        assert!(
            hint.contains("proof obligations failed") || hint.contains("not found"),
            "hint should guide proof fix or install: {hint}"
        );
    }
}
