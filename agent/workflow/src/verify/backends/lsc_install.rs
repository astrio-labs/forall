use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::agent_home::resolve_forall_home;

/// Pinned LemmaScript version (keep in sync with `agent/workflow/package.json`).
pub const LEMMASCRIPT_VERSION: &str = "0.5.7";

#[derive(Debug, Clone)]
pub struct EnsureLscResult {
    pub ok: bool,
    pub already_installed: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub script_path: Option<PathBuf>,
    pub wrapper_path: Option<PathBuf>,
    pub message: String,
}

pub fn lsc_install_hint() -> String {
    "LemmaScript not found. Run `forall ensure-lsc` to install under ~/.forall/tools/lemmascript/, or set FORALL_LSC=\"node /path/to/lsc.js\".".to_string()
}

pub fn forall_lsc_install_root() -> PathBuf {
    resolve_forall_home().join("tools").join("lemmascript")
}

pub fn forall_lsc_script() -> PathBuf {
    forall_lsc_install_root()
        .join("node_modules")
        .join("lemmascript")
        .join("tools")
        .join("dist")
        .join("lsc.js")
}

pub fn forall_lsc_wrapper() -> PathBuf {
    resolve_forall_home().join("bin").join("lsc")
}

/// The version actually on disk, read from the installed package.
///
/// `lsc` has no `--version` flag, so the manifest npm wrote is the only thing
/// that can be observed rather than assumed. This must never fall back to
/// [`LEMMASCRIPT_VERSION`]: that constant is the version we ask npm to
/// install, not evidence of what answered.
pub fn installed_lsc_version() -> Option<String> {
    installed_lsc_version_at(&forall_lsc_install_root())
}

/// [`installed_lsc_version`] against an explicit install root, so it can be
/// tested without setting a process-global environment variable.
pub fn installed_lsc_version_at(install_root: &Path) -> Option<String> {
    let manifest_path = install_root
        .join("node_modules")
        .join("lemmascript")
        .join("package.json");
    let raw = fs::read_to_string(manifest_path).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let version = manifest.get("version")?.as_str()?.trim();
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

/// The `FORALL_LSC` override, if it is set to something non-empty.
///
/// Lives here rather than in the runner so that availability, version and
/// execution all consult one definition of "which LemmaScript".
pub fn configured_lsc_override() -> Option<String> {
    let raw = std::env::var("FORALL_LSC").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Whether a usable LemmaScript is available.
///
/// An override is taken at its word: the user pointed at a LemmaScript
/// deliberately, and Forall neither installed it nor can inspect it. Checking
/// the managed install root instead would report "unavailable" while
/// [`super::lemmascript::resolve_lsc_command`] happily runs the override.
///
/// For the managed install both halves are required. The entry script alone
/// was accepted before, which meant a half-unpacked install reported
/// "available" and was then handed work it could not do. Requiring the
/// manifest to parse as well is the closest thing to a liveness probe
/// available for a tool with no version flag.
pub fn lsc_is_available() -> bool {
    if configured_lsc_override().is_some() {
        return true;
    }
    forall_lsc_script().is_file() && installed_lsc_version().is_some()
}

/// Toolchain identity for the ledger and for certificates.
///
/// `None` when the version cannot be observed, which is the contract
/// [`BackendRunResult::tool_version`] states: none means the probe failed.
/// This used to return `lemmascript@{LEMMASCRIPT_VERSION}` — a compile-time
/// constant — whenever the entry script merely existed, so every LemmaScript
/// verdict carried a version Forall asserted rather than observed, and a user
/// whose installed copy had drifted would have been signed for a version they
/// were not running.
///
/// The version is read from whichever LemmaScript
/// [`super::lemmascript::resolve_lsc_command`] would run, not from the managed
/// install root. Those differ whenever `FORALL_LSC` is set, and reading the
/// managed manifest then names a version that did not produce the verdict —
/// the same defect as the constant, one step removed.
pub fn lsc_version_check() -> Option<String> {
    let script = resolved_lsc_script()?;
    Some(format!("lemmascript@{}", lemmascript_version_near(&script)?))
}

/// The script that the resolved `lsc` command will execute.
///
/// `FORALL_LSC` is documented as `node /path/to/lsc.js`, so the script is an
/// argument rather than the command itself; a bare command is only a script if
/// it names a real file. An override that runs an opaque wrapper has no script
/// to inspect and yields `None`, which reports the version as unobservable
/// rather than guessing one.
fn resolved_lsc_script() -> Option<PathBuf> {
    let (command, args) = super::lemmascript::resolve_lsc_command();
    // The first script argument, because that is the one node runs — anything
    // after it is handed to the script rather than executed. Taking the last
    // would read the version beside whatever `FORALL_LSC="node lsc.js --config
    // tools.js"` happened to name at the end, which node never runs.
    if let Some(script) = args
        .iter()
        .find(|arg| arg.ends_with(".js") || arg.ends_with(".mjs") || arg.ends_with(".cjs"))
    {
        return Some(PathBuf::from(script));
    }
    let command = PathBuf::from(command);
    command.is_file().then_some(command)
}

/// The version in the `lemmascript` package manifest that owns `script`.
///
/// Walks up from the script because the layout differs between an npm install
/// (`node_modules/lemmascript/tools/dist/lsc.js`) and a source checkout, and
/// matches on the package name so that an unrelated `package.json` higher up
/// the tree cannot be mistaken for LemmaScript's own.
fn lemmascript_version_near(script: &Path) -> Option<String> {
    let script = fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf());
    for ancestor in script.ancestors().skip(1) {
        let Ok(raw) = fs::read_to_string(ancestor.join("package.json")) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if manifest.get("name").and_then(serde_json::Value::as_str) != Some("lemmascript") {
            continue;
        }
        let version = manifest.get("version")?.as_str()?.trim();
        if version.is_empty() {
            return None;
        }
        return Some(version.to_string());
    }
    None
}

pub fn project_has_typescript_sources(root: &Path) -> bool {
    let candidates = [
        root.join("src"),
        root.join("app"),
        root.join("lib"),
        root.to_path_buf(),
    ];
    for dir in candidates {
        if dir.is_dir() && dir_contains_ts(&dir, 0) {
            return true;
        }
    }
    false
}

fn dir_contains_ts(dir: &Path, depth: usize) -> bool {
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
            if name == "node_modules" || name == "target" || name.starts_with('.') {
                continue;
            }
            if dir_contains_ts(&path, depth + 1) {
                return true;
            }
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ts") | Some("tsx")
        ) {
            return true;
        }
    }
    false
}

fn write_package_json(install_root: &Path) -> io::Result<()> {
    fs::create_dir_all(install_root)?;
    let package_json = format!(
        r#"{{
  "name": "forall-lemmascript",
  "private": true,
  "description": "LemmaScript toolchain for Forall",
  "dependencies": {{
    "lemmascript": "{LEMMASCRIPT_VERSION}"
  }}
}}
"#
    );
    fs::write(install_root.join("package.json"), package_json)
}

fn write_lsc_wrapper(wrapper_path: &Path, script_path: &Path) -> io::Result<()> {
    if let Some(parent) = wrapper_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
NODE="${{NODE:-node}}"
LSC_JS="{lsc_js}"
exec "$NODE" "$LSC_JS" "$@"
"#,
        lsc_js = script_path.display(),
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

fn npm_install(install_root: &Path, quiet: bool) -> io::Result<()> {
    let npm = which_command("npm").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "npm not found — install Node.js (https://nodejs.org/) first",
        )
    })?;
    let output = Command::new(&npm)
        .args(["install", "--no-audit", "--no-fund", "--omit=dev"])
        .current_dir(install_root)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!("npm install failed: {stderr}")));
    }
    if !quiet {
        eprintln!(
            "forall ensure-lsc: installed lemmascript@{LEMMASCRIPT_VERSION} to {}",
            install_root.display()
        );
    }
    Ok(())
}

fn which_command(name: &str) -> Option<String> {
    Command::new("which")
        .arg(name)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Install LemmaScript under `$FORALL_HOME/tools/lemmascript/` and expose `~/.forall/bin/lsc`.
pub fn ensure_lsc_toolchain(allow_download: bool, quiet: bool) -> EnsureLscResult {
    if lsc_is_available() {
        return EnsureLscResult {
            ok: true,
            already_installed: true,
            installed: false,
            version: lsc_version_check(),
            script_path: Some(forall_lsc_script()),
            wrapper_path: Some(forall_lsc_wrapper()),
            message: "LemmaScript already available".to_string(),
        };
    }

    if !allow_download {
        return EnsureLscResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            script_path: None,
            wrapper_path: None,
            message: lsc_install_hint(),
        };
    }

    let install_root = forall_lsc_install_root();
    if let Err(err) = write_package_json(&install_root) {
        return EnsureLscResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            script_path: None,
            wrapper_path: None,
            message: format!("failed to prepare LemmaScript install dir: {err}"),
        };
    }

    if !forall_lsc_script().is_file()
        && let Err(err) = npm_install(&install_root, quiet)
    {
        return EnsureLscResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            script_path: None,
            wrapper_path: None,
            message: err.to_string(),
        };
    }

    let script = forall_lsc_script();
    if !script.is_file() {
        return EnsureLscResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            script_path: None,
            wrapper_path: None,
            message: format!(
                "LemmaScript install finished but lsc.js missing (expected {})",
                script.display()
            ),
        };
    }

    let wrapper = forall_lsc_wrapper();
    if let Err(err) = write_lsc_wrapper(&wrapper, &script) {
        return EnsureLscResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            script_path: Some(script),
            wrapper_path: None,
            message: format!("failed to write lsc wrapper: {err}"),
        };
    }

    EnsureLscResult {
        ok: true,
        already_installed: false,
        installed: true,
        version: lsc_version_check(),
        script_path: Some(script),
        wrapper_path: Some(wrapper),
        message: format!(
            "installed LemmaScript {LEMMASCRIPT_VERSION} to {}",
            install_root.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_json_names_lemmascript_dependency() {
        let dir = tempfile::tempdir().unwrap();
        write_package_json(dir.path()).unwrap();
        let raw = fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(raw.contains("lemmascript"));
        assert!(raw.contains(LEMMASCRIPT_VERSION));
    }

    fn install_root_with(manifest: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join("node_modules").join("lemmascript");
        fs::create_dir_all(&pkg).unwrap();
        if let Some(body) = manifest {
            fs::write(pkg.join("package.json"), body).unwrap();
        }
        dir
    }

    /// The toolchain identity behind a verdict must be read from the install,
    /// never asserted from a compile-time constant. This used to report
    /// `lemmascript@{LEMMASCRIPT_VERSION}` whenever the entry script merely
    /// existed, so a signed certificate could name a version that never ran.
    #[test]
    fn the_version_is_read_from_the_installed_package() {
        let dir = install_root_with(Some(r#"{"name":"lemmascript","version":"9.9.9"}"#));
        assert_eq!(
            installed_lsc_version_at(dir.path()).as_deref(),
            Some("9.9.9"),
            "must report what is installed, not the pinned constant"
        );
        assert_ne!(
            installed_lsc_version_at(dir.path()).as_deref(),
            Some(LEMMASCRIPT_VERSION)
        );
    }

    /// An unobservable version is `None`, which is exactly what
    /// `BackendRunResult::tool_version` documents: the probe failed. Asserting
    /// a value here would be worse than admitting we do not know.
    #[test]
    fn an_unreadable_install_reports_no_version_rather_than_guessing() {
        assert_eq!(
            installed_lsc_version_at(install_root_with(None).path()),
            None
        );
        assert_eq!(
            installed_lsc_version_at(install_root_with(Some("{ not json")).path()),
            None
        );
        assert_eq!(
            installed_lsc_version_at(install_root_with(Some(r#"{"name":"lemmascript"}"#)).path()),
            None,
            "a manifest without a version field is not a version"
        );
        assert_eq!(
            installed_lsc_version_at(install_root_with(Some(r#"{"version":"  "}"#)).path()),
            None,
            "whitespace is not a version"
        );
    }

    #[test]
    fn detects_typescript_in_src_tree() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.ts"), "export {}\n").unwrap();
        assert!(project_has_typescript_sources(dir.path()));
    }

    /// Lays out `<root>/<pkg-dir>/tools/dist/lsc.js` with a manifest beside
    /// the package root, which is how both npm and a source checkout arrange
    /// LemmaScript.
    fn checkout_with(pkg_dir: &str, manifest: Option<&str>) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let pkg = dir.path().join(pkg_dir);
        let script = pkg.join("tools").join("dist").join("lsc.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "// lsc\n").unwrap();
        if let Some(body) = manifest {
            fs::write(pkg.join("package.json"), body).unwrap();
        }
        (dir, script)
    }

    /// The version must describe whichever LemmaScript actually runs. A
    /// `FORALL_LSC` override runs a copy Forall did not install, so reading
    /// the managed install root would put a version in the ledger and in the
    /// signed certificate that did not produce the verdict.
    #[test]
    fn the_version_comes_from_the_package_that_owns_the_script() {
        let (_guard, script) =
            checkout_with("lemmascript", Some(r#"{"name":"lemmascript","version":"1.2.3"}"#));
        assert_eq!(
            lemmascript_version_near(&script).as_deref(),
            Some("1.2.3"),
            "an override's own manifest is the only honest source"
        );
    }

    /// An unrelated `package.json` further up the tree is not LemmaScript's.
    /// Without the name check, an override living under any Node project would
    /// be signed with that project's version.
    #[test]
    fn a_foreign_manifest_is_not_mistaken_for_lemmascript() {
        let (guard, script) = checkout_with("vendored", None);
        fs::write(
            guard.path().join("package.json"),
            r#"{"name":"someone-elses-app","version":"7.7.7"}"#,
        )
        .unwrap();
        assert_eq!(
            lemmascript_version_near(&script),
            None,
            "unknown is the correct answer; 7.7.7 would be a lie"
        );
    }

    #[test]
    fn an_unreadable_manifest_reports_no_version() {
        let (_guard, script) = checkout_with("lemmascript", Some("{ not json"));
        assert_eq!(lemmascript_version_near(&script), None);
        let (_guard, script) = checkout_with("lemmascript", Some(r#"{"name":"lemmascript"}"#));
        assert_eq!(lemmascript_version_near(&script), None);
    }
}
