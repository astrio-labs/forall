//! Frama-C toolchain discovery and wrapper management.
//!
//! - `FORALL_FRAMAC` env override
//! - `~/.forall/bin/frama-c` wrapper (exec script)
//! - `~/.forall/tools/framac-install/source.txt` link metadata
//! - `frama-c` on PATH
//!
//! Like OpenJML, Frama-C is not auto-downloaded — the blessed install path is
//! opam (`opam install frama-c`), so Forall only links an existing local install.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent_home::resolve_forall_home;

#[derive(Debug, Clone)]
pub struct EnsureFramacResult {
    pub ok: bool,
    pub already_installed: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub wrapper_path: Option<PathBuf>,
    pub message: String,
}

pub fn framac_install_hint() -> String {
    "Frama-C not found. Run `forall ensure-framac --path <frama-c binary or install dir>` to link a local install (e.g. `opam install frama-c`), or set FORALL_FRAMAC / FORALL_FRAMAC_HOME.".to_string()
}

pub fn forall_framac_wrapper() -> PathBuf {
    resolve_forall_home().join("bin").join("frama-c")
}

/// Forall-managed metadata directory (source.txt).
pub fn forall_framac_install_root() -> PathBuf {
    resolve_forall_home().join("tools").join("framac-install")
}

fn forall_framac_source_marker() -> PathBuf {
    forall_framac_install_root().join("source.txt")
}

pub fn framac_is_available() -> bool {
    framac_version_check().is_some()
}

/// Probe the resolved Frama-C command with `-version`.
pub fn framac_version_check() -> Option<String> {
    let (cmd, args) = resolve_framac_command_for_probe();
    let output = Command::new(&cmd)
        .args(&args)
        .arg("-version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first_line = text.lines().next()?.trim();
    if first_line.is_empty() {
        Some("frama-c".to_string())
    } else {
        Some(first_line.to_string())
    }
}

/// Resolve which `frama-c` executable Forall should invoke.
pub fn resolve_framac_command() -> (String, Vec<String>) {
    resolve_framac_command_for_probe()
}

fn resolve_framac_command_for_probe() -> (String, Vec<String>) {
    if let Ok(explicit) = std::env::var("FORALL_FRAMAC") {
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
    if let Some(binary) = find_framac_binary() {
        return (binary.display().to_string(), Vec::new());
    }
    let wrapper = forall_framac_wrapper();
    if wrapper.is_file() {
        return (wrapper.display().to_string(), Vec::new());
    }
    ("frama-c".to_string(), Vec::new())
}

fn write_framac_wrapper(wrapper_path: &Path, framac_bin: &Path) -> io::Result<()> {
    if let Some(parent) = wrapper_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if wrapper_path.is_symlink() || wrapper_path.is_file() {
        fs::remove_file(wrapper_path)?;
    }
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nexec {framac_bin:?} \"$@\"\n",
        framac_bin = framac_bin.display()
    );
    fs::write(wrapper_path, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(wrapper_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(wrapper_path, perms)?;
    }
    Ok(())
}

fn read_source_marker(marker: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(marker).ok()?;
    let path = PathBuf::from(raw.trim());
    if path.is_file() { Some(path) } else { None }
}

fn is_forall_managed_framac(path: &Path) -> bool {
    if path == forall_framac_wrapper() {
        return true;
    }
    if path.starts_with(forall_framac_install_root()) {
        return true;
    }
    if let (Ok(a), Ok(b)) = (path.canonicalize(), forall_framac_wrapper().canonicalize())
        && a == b
    {
        return true;
    }
    false
}

fn find_framac_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("FORALL_FRAMAC") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let cmd = trimmed.split_whitespace().next()?.to_string();
            let path = PathBuf::from(&cmd);
            if path.is_file() && !is_forall_managed_framac(&path) {
                return Some(path);
            }
        }
    }
    if let Some(linked) = read_source_marker(&forall_framac_source_marker()) {
        return Some(linked);
    }
    for candidate in default_framac_candidates() {
        if let Some(script) = resolve_framac_script(&candidate) {
            return Some(script);
        }
    }
    super::path_lookup("frama-c").filter(|path| !is_forall_managed_framac(path))
}

/// Accept a `frama-c` binary directly, or an install dir holding
/// `frama-c` / `bin/frama-c` (opam switch layout).
fn resolve_framac_script(source: &Path) -> Option<PathBuf> {
    if source.is_file() {
        return Some(source.to_path_buf());
    }
    [source.join("frama-c"), source.join("bin").join("frama-c")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

fn default_framac_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("FORALL_FRAMAC_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }
    candidates
}

/// `compile_commands.json` at the project root, for `-json-compilation-database`.
pub fn compile_commands_arg(root: &Path) -> Option<String> {
    let db = root.join("compile_commands.json");
    if db.is_file() {
        Some(db.display().to_string())
    } else {
        None
    }
}

/// Discover C header roots under a project (for `-cpp-extra-args=-I...`).
pub fn detect_c_include_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for name in ["include", "src", "lib"] {
        let candidate = root.join(name);
        if candidate.is_dir() && dir_contains_c_headers(&candidate, 0) {
            paths.push(candidate);
        }
    }
    if paths.is_empty() && dir_contains_c_headers(root, 0) {
        paths.push(root.to_path_buf());
    }
    paths
}

/// Single `-cpp-extra-args=-I<dir> -I<dir>` argument for Frama-C, if any
/// header roots exist. One argv entry — Frama-C splits the value itself.
pub fn cpp_include_arg(root: &Path) -> Option<String> {
    let paths: Vec<String> = detect_c_include_paths(root)
        .into_iter()
        .map(|p| format!("-I{}", p.display()))
        .collect();
    if paths.is_empty() {
        None
    } else {
        Some(format!("-cpp-extra-args={}", paths.join(" ")))
    }
}

pub fn project_has_c_sources(root: &Path) -> bool {
    let candidates = [root.join("src"), root.join("lib"), root.to_path_buf()];
    for dir in candidates {
        if dir.is_dir() && dir_contains_c_files(&dir, 0) {
            return true;
        }
    }
    false
}

fn dir_contains_c_files(dir: &Path, depth: usize) -> bool {
    dir_contains_ext(dir, depth, "c")
}

fn dir_contains_c_headers(dir: &Path, depth: usize) -> bool {
    dir_contains_ext(dir, depth, "h")
}

fn dir_contains_ext(dir: &Path, depth: usize, ext: &str) -> bool {
    if depth > 6 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(
                name,
                "target" | "build" | "node_modules" | ".git" | ".forall"
            ) {
                continue;
            }
            if dir_contains_ext(&path, depth + 1, ext) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            return true;
        }
    }
    false
}

/// Point Forall at a Frama-C install directory or `frama-c` binary.
pub fn link_framac_from(source: &Path) -> io::Result<PathBuf> {
    let script = resolve_framac_script(source).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no frama-c binary found at {} (expected frama-c file, <dir>/frama-c, or <dir>/bin/frama-c)",
                source.display()
            ),
        )
    })?;
    write_framac_wrapper(&forall_framac_wrapper(), &script)?;
    let install_dir = forall_framac_install_root();
    fs::create_dir_all(&install_dir)?;
    fs::write(install_dir.join("source.txt"), script.display().to_string())?;
    Ok(script)
}

/// Link or refresh `~/.forall/bin/frama-c` when a binary is discoverable.
///
/// When `allow_link` is true, tries linking from `FORALL_FRAMAC_HOME` candidates.
/// Like OpenJML, Frama-C is not auto-downloaded — only linked from a local install.
pub fn ensure_framac_toolchain(allow_link: bool, quiet: bool) -> EnsureFramacResult {
    let initially_available = framac_version_check().is_some();

    if let Some(binary) = find_framac_binary() {
        if let Err(err) = write_framac_wrapper(&forall_framac_wrapper(), &binary)
            && !quiet
        {
            eprintln!("forall ensure-framac: warning: could not refresh wrapper: {err}");
        }
    } else if allow_link {
        for candidate in default_framac_candidates() {
            if resolve_framac_script(&candidate).is_some() && link_framac_from(&candidate).is_ok() {
                if !quiet {
                    eprintln!(
                        "forall ensure-framac: linked Frama-C from {}",
                        candidate.display()
                    );
                }
                break;
            }
        }
    }

    if let Some(version) = framac_version_check() {
        return EnsureFramacResult {
            ok: true,
            already_installed: initially_available,
            installed: !initially_available,
            version: Some(version),
            wrapper_path: Some(forall_framac_wrapper()),
            message: if initially_available {
                "Frama-C already available".to_string()
            } else {
                format!("linked Frama-C at {}", forall_framac_wrapper().display())
            },
        };
    }

    EnsureFramacResult {
        ok: false,
        already_installed: false,
        installed: false,
        version: None,
        wrapper_path: None,
        message: framac_install_hint(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_framac_binary_skips_forall_wrapper() {
        let wrapper = forall_framac_wrapper();
        assert!(is_forall_managed_framac(&wrapper));
        if let Some(binary) = find_framac_binary() {
            assert!(
                !is_forall_managed_framac(&binary),
                "must not resolve to Forall wrapper: {}",
                binary.display()
            );
        }
    }

    #[test]
    fn install_root_uses_framac_install_dir() {
        let root = forall_framac_install_root();
        assert!(root.ends_with("framac-install"));
    }

    #[test]
    fn detect_c_include_paths_finds_include_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("include/util")).unwrap();
        std::fs::write(root.join("include/util/math.h"), "int clamp(int x);\n").unwrap();
        let paths = detect_c_include_paths(root);
        assert_eq!(paths, vec![root.join("include")]);
        let arg = cpp_include_arg(root).expect("cpp include arg");
        assert!(arg.starts_with("-cpp-extra-args="));
        assert!(arg.contains("-I"));
        assert!(arg.contains("include"));
    }

    #[test]
    fn compile_commands_arg_requires_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(compile_commands_arg(dir.path()).is_none());
        std::fs::write(dir.path().join("compile_commands.json"), "[]").unwrap();
        assert!(compile_commands_arg(dir.path()).is_some());
    }
}
