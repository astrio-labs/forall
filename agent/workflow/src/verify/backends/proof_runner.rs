//! Shared proof command execution with optional timeouts.
//!
//! Env resolution (first match wins):
//! - Backend-specific: `FORALL_OPENJML_TIMEOUT_SECS`, `FORALL_LSC_TIMEOUT_SECS`, `FORALL_VERUS_TIMEOUT_SECS`, `FORALL_FRAMAC_TIMEOUT_SECS`
//! - Global fallback: `FORALL_PROOF_TIMEOUT_SECS`
//! - Built-in default per backend
//!
//! Set any timeout env var to `0` to disable the limit for that scope.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::BackendRunResult;

/// Per-file OpenJML ESC — scalar proofs usually finish quickly; 180s covers heavier files.
pub const DEFAULT_OPENJML_TIMEOUT_SECS: u64 = 180;
/// Per-file LemmaScript/Dafny check.
pub const DEFAULT_LSC_TIMEOUT_SECS: u64 = 180;
/// Whole-crate `cargo verus verify` — allow more time than per-file backends.
pub const DEFAULT_VERUS_TIMEOUT_SECS: u64 = 600;
/// Per-file Frama-C WP — SMT dispatch over many goals; slower than ESC on average.
pub const DEFAULT_FRAMAC_TIMEOUT_SECS: u64 = 300;

fn parse_timeout_env(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn resolve_timeout_secs(specific_var: &str, default_secs: u64) -> Option<u64> {
    if let Some(value) = parse_timeout_env(specific_var) {
        return if value == 0 { None } else { Some(value) };
    }
    if let Some(value) = parse_timeout_env("FORALL_PROOF_TIMEOUT_SECS") {
        return if value == 0 { None } else { Some(value) };
    }
    if default_secs == 0 {
        None
    } else {
        Some(default_secs)
    }
}

pub fn openjml_proof_timeout_secs() -> Option<u64> {
    resolve_timeout_secs("FORALL_OPENJML_TIMEOUT_SECS", DEFAULT_OPENJML_TIMEOUT_SECS)
}

pub fn lsc_proof_timeout_secs() -> Option<u64> {
    resolve_timeout_secs("FORALL_LSC_TIMEOUT_SECS", DEFAULT_LSC_TIMEOUT_SECS)
}

pub fn verus_proof_timeout_secs() -> Option<u64> {
    resolve_timeout_secs("FORALL_VERUS_TIMEOUT_SECS", DEFAULT_VERUS_TIMEOUT_SECS)
}

pub fn framac_proof_timeout_secs() -> Option<u64> {
    resolve_timeout_secs("FORALL_FRAMAC_TIMEOUT_SECS", DEFAULT_FRAMAC_TIMEOUT_SECS)
}

fn read_child_output(
    stdout: &mut Option<std::process::ChildStdout>,
    stderr: &mut Option<std::process::ChildStderr>,
) -> (String, String) {
    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut pipe) = stdout.take() {
        let _ = pipe.read_to_string(&mut out);
    }
    if let Some(mut pipe) = stderr.take() {
        let _ = pipe.read_to_string(&mut err);
    }
    (out, err)
}

/// Run a proof subprocess, optionally killing it after `timeout_secs`.
pub fn run_command_with_timeout(
    cmd: &str,
    args: &[String],
    cwd: &Path,
    timeout_secs: Option<u64>,
    timeout_label: &str,
) -> BackendRunResult {
    run_command_with_timeout_and_path(cmd, args, cwd, timeout_secs, timeout_label, None)
}

/// As [`run_command_with_timeout`], with a directory prepended to the child's
/// `PATH`.
///
/// A backend that shells out to another tool needs its dependency to be
/// findable by the child, not merely present on disk. Prepending rather than
/// replacing means a tool the user already has on PATH still wins.
pub fn run_command_with_timeout_and_path(
    cmd: &str,
    args: &[String],
    cwd: &Path,
    timeout_secs: Option<u64>,
    timeout_label: &str,
    extra_path: Option<std::path::PathBuf>,
) -> BackendRunResult {
    let command_label = format!("{cmd} {}", args.join(" "));

    let mut command = Command::new(cmd);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = extra_path {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let mut entries = vec![dir];
        entries.extend(std::env::split_paths(&existing));
        if let Ok(joined) = std::env::join_paths(entries) {
            command.env("PATH", joined);
        }
    }
    crate::verify::command_security::apply_hosted_identity(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return BackendRunResult {
                ok: false,
                code: None,
                stdout: String::new(),
                stderr: err.to_string(),
                command: command_label,
                timed_out: false,
                tool_version: None,
            };
        }
    };

    let Some(timeout_secs) = timeout_secs else {
        match child.wait_with_output() {
            Ok(out) => {
                return BackendRunResult {
                    ok: out.status.success(),
                    code: out.status.code(),
                    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                    command: command_label,
                    timed_out: false,
                    tool_version: None,
                };
            }
            Err(err) => {
                return BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: err.to_string(),
                    command: command_label,
                    timed_out: false,
                    tool_version: None,
                };
            }
        }
    };

    let timeout = Duration::from_secs(timeout_secs);
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, stderr) = read_child_output(&mut child.stdout, &mut child.stderr);
                return BackendRunResult {
                    ok: status.success(),
                    code: status.code(),
                    stdout,
                    stderr,
                    command: command_label,
                    timed_out: false,
                    tool_version: None,
                };
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let (stdout, stderr) = read_child_output(&mut child.stdout, &mut child.stderr);
                let mut stderr = stderr;
                if !stdout.is_empty() {
                    if !stderr.is_empty() {
                        stderr.push('\n');
                    }
                    stderr.push_str(&stdout);
                }
                if stderr.is_empty() {
                    stderr = format!("{timeout_label} timed out after {timeout_secs}s");
                }
                return BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr,
                    command: command_label,
                    timed_out: true,
                    tool_version: None,
                };
            }
            Err(err) => {
                return BackendRunResult {
                    ok: false,
                    code: None,
                    stdout: String::new(),
                    stderr: err.to_string(),
                    command: command_label,
                    timed_out: false,
                    tool_version: None,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_backend_defaults_differ_by_workload() {
        assert_eq!(DEFAULT_OPENJML_TIMEOUT_SECS, 180);
        assert_eq!(DEFAULT_LSC_TIMEOUT_SECS, 180);
        assert_eq!(DEFAULT_VERUS_TIMEOUT_SECS, 600);
    }
}
