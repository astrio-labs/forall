//! Shared proof command execution with optional timeouts.
//!
//! Env resolution (first match wins):
//! - Backend-specific: `FORALL_OPENJML_TIMEOUT_SECS`, `FORALL_LSC_TIMEOUT_SECS`, `FORALL_VERUS_TIMEOUT_SECS`, `FORALL_FRAMAC_TIMEOUT_SECS`
//! - Global fallback: `FORALL_PROOF_TIMEOUT_SECS`
//! - Built-in default per backend
//!
//! Set any timeout env var to `0` to disable the limit for that scope.

use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
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

/// Capture timed commands in separate temporary files so a full pipe cannot
/// block the prover. A descendant holding a writer open cannot delay EOF.
fn timed_output(command: &mut Command, timeout: Duration) -> io::Result<(Output, bool)> {
    let stdout = tempfile::NamedTempFile::new()?;
    let stderr = tempfile::NamedTempFile::new()?;
    command.stdout(stdout.reopen()?).stderr(stderr.reopen()?);
    let mut child = command.spawn()?;
    let started = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status, false),
            Ok(None) if started.elapsed() < timeout => {
                thread::sleep(
                    Duration::from_millis(50).min(timeout.saturating_sub(started.elapsed())),
                );
            }
            Ok(None) => {
                child.kill()?;
                break (child.wait()?, true);
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        }
    };

    // Each reader has its own cursor. Snapshot its length in case a descendant
    // is still writing after the command exits.
    let read = |file: &std::fs::File| -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        file.take(file.metadata()?.len()).read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    Ok((
        Output {
            status,
            stdout: read(stdout.as_file())?,
            stderr: read(stderr.as_file())?,
        },
        timed_out,
    ))
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
    let output = match timeout_secs {
        Some(secs) => timed_output(&mut command, Duration::from_secs(secs)),
        None => command
            .spawn()
            .and_then(|child| child.wait_with_output())
            .map(|out| (out, false)),
    };
    let (out, timed_out) = match output {
        Ok(result) => result,
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
    let mut stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if timed_out {
        if !stdout.is_empty() {
            if !stderr.is_empty() {
                stderr.push('\n');
            }
            stderr.push_str(&stdout);
            stdout.clear();
        }
        if stderr.is_empty()
            && let Some(secs) = timeout_secs
        {
            stderr = format!("{timeout_label} timed out after {secs}s");
        }
    }
    BackendRunResult {
        ok: !timed_out && out.status.success(),
        code: if timed_out { None } else { out.status.code() },
        stdout,
        stderr,
        command: command_label,
        timed_out,
        tool_version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn timed_command_captures_large_output_from_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_command_with_timeout(
            "sh",
            &["-c".into(), "dd if=/dev/zero bs=1024 count=1024 2>/dev/null; dd if=/dev/zero bs=1024 count=1024 >&2 2>/dev/null".into()],
            dir.path(),
            Some(5),
            "test prover",
        );
        assert!(result.ok);
        assert!(!result.timed_out);
        assert_eq!(result.stdout.len(), 1024 * 1024);
        assert_eq!(result.stderr.len(), 1024 * 1024);
    }

    #[cfg(unix)]
    #[test]
    fn timed_out_command_preserves_partial_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let result = run_command_with_timeout(
            "sh",
            &[
                "-c".into(),
                "printf progress; printf error >&2; exec sleep 30".into(),
            ],
            dir.path(),
            Some(1),
            "test prover",
        );
        assert!(!result.ok);
        assert!(result.timed_out);
        assert_eq!(result.code, None);
        assert_eq!(result.stderr, "error\nprogress");
    }

    #[test]
    fn per_backend_defaults_differ_by_workload() {
        assert_eq!(DEFAULT_OPENJML_TIMEOUT_SECS, 180);
        assert_eq!(DEFAULT_LSC_TIMEOUT_SECS, 180);
        assert_eq!(DEFAULT_VERUS_TIMEOUT_SECS, 600);
    }
}
