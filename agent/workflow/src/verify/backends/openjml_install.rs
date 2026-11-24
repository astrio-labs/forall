//! OpenJML toolchain provisioning, discovery and wrapper management.
//!
//! Resolution order:
//!
//! - `FORALL_OPENJML` env override
//! - `~/.forall/tools/openjml-install/source.txt` link metadata
//! - `~/.forall/tools/openjml/<tag>/` releases Forall installed
//! - `FORALL_OPENJML_HOME` candidates
//! - `openjml` on PATH
//!
//! Installs land under a per-tool root, `~/.forall/tools/openjml/<tag>/`, the
//! layout Verus and Dafny already use. That matters more here than for either
//! of them, because the OpenJML archive unpacks *flat*: no top-level directory,
//! just its ~30 entries — `jdk/`, `specs/`, `demos/`, `lsp/`, `tutorial/`,
//! `README`, the `openjml` launcher and its siblings. Unpacked into the shared
//! `tools/` root those names collide with any other toolchain's, and nothing
//! distinguishes them from the root's own contents, so there is no way to
//! upgrade or remove the install. Under `tools/openjml/<tag>/` a release is one
//! removable directory and two of them can coexist.
//!
//! [`migrate_legacy_flat_install`] moves a pre-existing flat unpack into that
//! layout, which is also what frees the name: `tools/openjml` is a *file* (the
//! launcher) on those machines, so the directory cannot be created until it moves.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::agent_home::resolve_forall_home;

/// Stable releases only, matching `verus_install` and `dafny_install`.
const RELEASES_API: &str = "https://api.github.com/repos/OpenJML/OpenJML/releases/latest";

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    /// Announced before the download starts: the archive bundles a JDK, the
    /// specs and the solvers, so every asset is 150 MB or more.
    size: u64,
    /// `sha256:<hex>`, served by the release API as a first-class field.
    ///
    /// Better provenance than Verus, which has to scrape the digest out of
    /// human-written release notes. Absent on releases published before GitHub
    /// added the field, so it is verified when present rather than required.
    #[serde(default)]
    digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnsureOpenjmlResult {
    pub ok: bool,
    pub already_installed: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub wrapper_path: Option<PathBuf>,
    /// The `tools/openjml/<tag>/` directory, when this run installed one.
    pub install_root: Option<PathBuf>,
    pub message: String,
}

pub fn openjml_install_hint() -> String {
    "OpenJML not found. Run `forall ensure-openjml` to install the official release under ~/.forall/tools/openjml/, pass --path <OpenJMLsrc/> to link a local install, or set FORALL_OPENJML / FORALL_OPENJML_HOME.".to_string()
}

pub fn forall_openjml_wrapper() -> PathBuf {
    resolve_forall_home().join("bin").join("openjml")
}

/// Forall-managed metadata directory (source.txt). Kept separate from user
/// installs that may place an `openjml` script at `~/.forall/tools/openjml`.
pub fn forall_openjml_install_root() -> PathBuf {
    resolve_forall_home().join("tools").join("openjml-install")
}

fn forall_openjml_source_marker() -> PathBuf {
    forall_openjml_install_root().join("source.txt")
}

fn legacy_openjml_source_marker() -> PathBuf {
    resolve_forall_home()
        .join("tools")
        .join("openjml")
        .join("source.txt")
}

/// Per-tool root holding the releases Forall installed, one directory per tag:
/// `~/.forall/tools/openjml/<tag>/`.
///
/// Distinct from [`forall_openjml_install_root`], which holds only link
/// metadata for an install Forall did not download.
pub fn forall_openjml_releases_root() -> PathBuf {
    resolve_forall_home().join("tools").join("openjml")
}

/// The `openjml` launcher inside an unpacked release.
///
/// The archive unpacks flat, so this is `<dir>/openjml`. One level of nesting
/// is tolerated as well: the `mcp-host` image locates the launcher with `find`
/// rather than a fixed path, so nothing upstream guarantees the flat shape
/// holds for every platform's asset.
fn launcher_in_release_dir(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("openjml");
    if direct.is_file() {
        return Some(direct);
    }
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let nested = entry.path().join("openjml");
        if nested.is_file() {
            return Some(nested);
        }
    }
    None
}

/// Every OpenJML launcher Forall has installed, newest release directory last.
fn installed_openjml_launchers() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(forall_openjml_releases_root()) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let path = entry.path();
        // Staging left behind by an interrupted install is not a release.
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'))
        {
            continue;
        }
        if let Some(launcher) = launcher_in_release_dir(&path) {
            let stamp = fs::metadata(&launcher)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            found.push((stamp, launcher));
        }
    }
    found.sort_by_key(|(stamp, _)| *stamp);
    found.into_iter().map(|(_, path)| path).collect()
}

pub fn openjml_is_available() -> bool {
    openjml_version_check().is_some()
}

/// Whether `FORALL_OPENJML` names something, in which case it wins at resolve
/// time and Forall must not try to install its way around it.
fn has_explicit_openjml_override() -> bool {
    std::env::var("FORALL_OPENJML").is_ok_and(|value| !value.trim().is_empty())
}

/// Run one specific launcher, rather than whatever resolution currently picks.
///
/// A liveness check: a release that unpacked badly, lost its execute bit or was
/// half-deleted is a file that exists and cannot prove anything.
fn probe_launcher(binary: &Path) -> Option<String> {
    let output = Command::new(binary).arg("-?").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first_line = text.lines().next()?.trim();
    if first_line.is_empty() {
        Some("openjml".to_string())
    } else {
        Some(first_line.to_string())
    }
}

/// Probe the resolved OpenJML command with `-?` (same as OpenJMLsrc/openjml help).
pub fn openjml_version_check() -> Option<String> {
    let (cmd, args) = resolve_openjml_command_for_probe();
    let output = Command::new(&cmd).args(&args).arg("-?").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let first_line = text.lines().next()?.trim();
    if first_line.is_empty() {
        Some("openjml".to_string())
    } else {
        Some(first_line.to_string())
    }
}

/// Resolve which `openjml` executable Forall should invoke.
pub fn resolve_openjml_command() -> (String, Vec<String>) {
    resolve_openjml_command_for_probe()
}

fn resolve_openjml_command_for_probe() -> (String, Vec<String>) {
    if let Ok(explicit) = std::env::var("FORALL_OPENJML") {
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
    if let Some(binary) = find_openjml_binary() {
        return (binary.display().to_string(), Vec::new());
    }
    let wrapper = forall_openjml_wrapper();
    if wrapper.is_file() {
        return (wrapper.display().to_string(), Vec::new());
    }
    ("openjml".to_string(), Vec::new())
}

fn write_openjml_wrapper(wrapper_path: &Path, openjml_bin: &Path) -> io::Result<()> {
    if let Some(parent) = wrapper_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if wrapper_path.is_symlink() || wrapper_path.is_file() {
        fs::remove_file(wrapper_path)?;
    }
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nexec {openjml_bin:?} \"$@\"\n",
        openjml_bin = openjml_bin.display()
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

fn read_linked_openjml_source() -> Option<PathBuf> {
    read_source_marker(&forall_openjml_source_marker())
        .or_else(|| read_source_marker(&legacy_openjml_source_marker()))
}

fn is_forall_managed_openjml(path: &Path) -> bool {
    if path == forall_openjml_wrapper() {
        return true;
    }
    if path.starts_with(forall_openjml_install_root()) {
        return true;
    }
    if let (Ok(a), Ok(b)) = (path.canonicalize(), forall_openjml_wrapper().canonicalize())
        && a == b
    {
        return true;
    }
    false
}

fn find_openjml_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("FORALL_OPENJML") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            let cmd = trimmed.split_whitespace().next()?.to_string();
            let path = PathBuf::from(&cmd);
            if path.is_file() && !is_forall_managed_openjml(&path) {
                return Some(path);
            }
        }
    }
    if let Some(linked) = read_linked_openjml_source() {
        return Some(linked);
    }
    // Releases Forall installed, newest first, so detection follows the same
    // per-tool path the installer writes to.
    if let Some(installed) = installed_openjml_launchers().last().cloned() {
        return Some(installed);
    }
    for candidate in default_openjml_candidates() {
        if let Some(script) = resolve_openjml_script(&candidate) {
            return Some(script);
        }
    }
    super::path_lookup("openjml").filter(|path| !is_forall_managed_openjml(path))
}

fn resolve_openjml_script(source: &Path) -> Option<PathBuf> {
    if source.is_file() {
        return Some(source.to_path_buf());
    }
    let script = source.join("openjml");
    if script.is_file() {
        return Some(script);
    }
    None
}

fn default_openjml_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("FORALL_OPENJML_HOME") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }
    candidates
}

/// Discover Java source roots under a project (for `openjml -sourcepath`).
pub fn detect_java_source_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for name in ["src/main/java", "src", "app", "java"] {
        let candidate = root.join(name);
        if candidate.is_dir() && dir_contains_java(&candidate, 0) {
            paths.push(candidate);
        }
    }
    if paths.is_empty() && dir_contains_java(root, 0) {
        paths.push(root.to_path_buf());
    }
    paths
}

/// Colon/semicolon-separated `-sourcepath` value for OpenJML.
pub fn java_sourcepath_arg(root: &Path) -> Option<String> {
    let paths: Vec<String> = detect_java_source_paths(root)
        .into_iter()
        .map(|p| p.display().to_string())
        .collect();
    if paths.is_empty() {
        None
    } else {
        let sep = if cfg!(windows) { ";" } else { ":" };
        Some(paths.join(sep))
    }
}

/// Point Forall at an OpenJML install directory (`OpenJMLsrc/`) or `openjml` script.
pub fn link_openjml_from(source: &Path) -> io::Result<PathBuf> {
    let script = resolve_openjml_script(source).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no openjml script found at {} (expected openjml file or <dir>/openjml)",
                source.display()
            ),
        )
    })?;
    write_openjml_wrapper(&forall_openjml_wrapper(), &script)?;
    let install_dir = forall_openjml_install_root();
    fs::create_dir_all(&install_dir)?;
    fs::write(install_dir.join("source.txt"), script.display().to_string())?;
    Ok(script)
}

pub fn project_has_java_sources(root: &Path) -> bool {
    let candidates = [root.join("src"), root.join("app"), root.to_path_buf()];
    for dir in candidates {
        if dir.is_dir() && dir_contains_java(&dir, 0) {
            return true;
        }
    }
    false
}

fn dir_contains_java(dir: &Path, depth: usize) -> bool {
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
            if dir_contains_java(&path, depth + 1) {
                return true;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("java") {
            return true;
        }
    }
    false
}

/// The `<os>-<arch>` fragment of the release asset name for this platform.
///
/// Matched as a fragment rather than a whole suffix, like `dafny_install`:
/// OpenJML stamps the CI runner's OS release into the Linux x86_64 asset
/// (`openjml-ubuntu-24.04-<tag>.zip`), and that moves between releases, so
/// pinning a suffix would break on the next runner upgrade.
fn platform_asset_fragment() -> Result<&'static str, String> {
    asset_fragment_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Split from [`platform_asset_fragment`] so every platform's answer is
/// testable from any host. `std::env::consts::OS` is fixed at compile time, so
/// without this only the building platform's arm is ever exercised.
fn asset_fragment_for(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os, arch) {
        ("macos", "aarch64") => Ok("-macos-arm64-"),
        ("macos", "x86_64") => Ok("-macos-x86_64-"),
        ("linux", "aarch64") => Ok("-ubuntu-arm64-"),
        ("linux", "x86_64") => Ok("-ubuntu-"),
        // Upstream *does* publish `openjml-windows-<tag>.zip`. The refusal is
        // Forall's: `write_openjml_wrapper` emits a bash script and marks it
        // executable only under `cfg(unix)`, so installing here would download
        // ~178 MB and then write a wrapper that cannot run. Saying that is
        // better than a wrapper that fails at the first proof.
        ("windows", _) => Err(
            "Forall cannot install OpenJML on Windows: the `openjml` wrapper it writes is a bash script. Upstream does publish a Windows build — install it yourself, then link it with `forall ensure-openjml --path <OpenJMLsrc/>`, or set FORALL_OPENJML."
                .to_string(),
        ),
        (os, arch) => Err(format!(
            "OpenJML does not publish a prebuilt toolchain for {os} {arch}. Link a local install with `forall ensure-openjml --path <OpenJMLsrc/>`, or set FORALL_OPENJML."
        )),
    }
}

fn find_platform_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    select_asset(assets, platform_asset_fragment().ok()?)
}

fn select_asset<'a>(assets: &'a [GithubAsset], fragment: &str) -> Option<&'a GithubAsset> {
    // `-ubuntu-` is also a prefix of the arm64 Linux asset, so the x86_64
    // build has to be told apart from it rather than matched loosely.
    let exclude_arm64 = fragment == "-ubuntu-";
    assets.iter().find(|asset| {
        asset.name.ends_with(".zip")
            && asset.name.contains(fragment)
            && !(exclude_arm64 && asset.name.contains("arm64"))
    })
}

/// Release tags and `version-info.txt` versions both become directory names,
/// so neither is allowed to escape the per-tool root.
fn is_safe_dir_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn curl_json(url: &str) -> io::Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: forall",
            url,
        ])
        .output()
        .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl failed fetching release metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn curl_download(url: &str, dest: &Path) -> io::Result<()> {
    let output = Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl failed downloading OpenJML asset: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let output = if cfg!(target_os = "macos") {
        Command::new("shasum")
            .args(["-a", "256"])
            .arg(path)
            .output()
    } else {
        Command::new("sha256sum").arg(path).output()
    }
    .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !output.status.success() {
        return Err(io::Error::other("sha256 checksum command failed"));
    }
    let line = String::from_utf8_lossy(&output.stdout);
    Ok(line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase())
}

/// The hex digest from an asset's `sha256:<hex>` field, when it has one.
fn expected_sha256(asset: &GithubAsset) -> Option<String> {
    let raw = asset.digest.as_deref()?.trim();
    let hex = raw.strip_prefix("sha256:")?.trim().to_ascii_lowercase();
    if hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(hex)
    } else {
        None
    }
}

/// Whether a downloaded archive may be unpacked, given the digest the release
/// announced and the digest computed locally.
///
/// Fails closed. The archive is unpacked and then *executed*, and hooks reach
/// the installer on their own, so "could not verify" must not degrade into
/// "install anyway" behind a warning no hook caller ever reads. Refusing costs
/// nothing in practice — every asset of the current release carries a digest —
/// and `--path` still links an install the user vouches for.
fn digest_verdict(expected: &str, computed: io::Result<String>) -> Result<(), String> {
    match computed {
        Ok(actual) if actual == expected => Ok(()),
        Ok(actual) => Err(format!(
            "OpenJML download checksum mismatch (expected {expected}, got {actual})"
        )),
        Err(err) => Err(format!(
            "cannot verify the OpenJML download: {err}. Install it yourself and link it with `forall ensure-openjml --path <OpenJMLsrc/>`."
        )),
    }
}

fn missing_digest_message(tag: &str, asset_name: &str) -> String {
    format!(
        "OpenJML release {tag} publishes no sha256 digest for {asset_name}, so the download cannot be verified. Install it yourself and link it with `forall ensure-openjml --path <OpenJMLsrc/>`."
    )
}

fn unzip_archive(archive: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    let output = Command::new("unzip")
        .args(["-oq"])
        .arg(archive)
        .arg("-d")
        .arg(dest)
        .output()
        .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "unzip failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// The top-level names an OpenJML release archive unpacks, taken from the
/// 21.0.27 assets.
///
/// An allowlist rather than a list of the neighbours to avoid, so that anything
/// unrecognised — a tool root added after this was written, a file the user put
/// there — stays where it is instead of being swallowed into the release
/// directory. A new entry in a future OpenJML archive is then merely left
/// behind in `tools/`, which is visible and harmless; the reverse mistake is
/// neither.
const OPENJML_RELEASE_ENTRIES: &[&str] = &[
    "JML_Reference_Manual.pdf",
    "OpenJMLUserGuide.pdf",
    "README",
    "demos",
    "install-vscode-extension",
    "jdk",
    "jmlruntime.jar",
    "lsp",
    "mac-setup",
    "os-name.sh",
    "setup-exports",
    "setup-exports.bat",
    "setup-vars",
    "setup-vars.bat",
    "specs",
    "tutorial",
    "version-info.txt",
];

/// Whether a `tools/` entry belongs to a flat OpenJML release.
///
/// The two prefixes cover the names that vary: `Solvers-macos-arm64` and its
/// per-platform siblings, and the `openjml*` launcher family (`openjml`,
/// `openjml-java`, `openjml.bat`, `openjml.vsix`, …). `openjml-install` is
/// Forall's own link metadata and shares that prefix, so it is excluded.
fn is_openjml_release_entry(name: &str) -> bool {
    if name == "openjml-install" {
        return false;
    }
    OPENJML_RELEASE_ENTRIES.contains(&name)
        || name.starts_with("Solvers-")
        || name.starts_with("openjml")
}

/// The version of an OpenJML release that was unpacked flat into `tools/`.
///
/// Gated on the launcher, `setup-vars` and `version-info.txt` all being
/// present, a combination that only arises when a release archive was unpacked
/// into this directory.
fn legacy_flat_openjml_version(tools: &Path) -> Option<String> {
    if !tools.join("openjml").is_file() || !tools.join("setup-vars").is_file() {
        return None;
    }
    let raw = fs::read_to_string(tools.join("version-info.txt")).ok()?;
    let version = raw.lines().next()?.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

fn undo_moves(moved: &[(PathBuf, PathBuf)]) {
    for (from, to) in moved.iter().rev() {
        let _ = fs::rename(to, from);
    }
}

/// Put a failed migration back and leave nothing of the release behind.
///
/// Order is load-bearing. `tools/openjml` is the launcher's original path, so
/// while the per-tool directory occupies that name the launcher cannot be
/// restored; it has to be removed *before* the moves are undone, not after.
///
/// Staging is then removed only if it is empty. Anything still inside is a
/// piece of the user's install that could not be put back, and deleting it
/// recursively would destroy the very install this was trying to reorganise —
/// a ~490 MB loss to tidy up after a failed rename.
fn abandon_migration(moved: &[(PathBuf, PathBuf)], releases: &Path, staging: &Path) {
    // Empty-only: a populated per-tool root belongs to real installs.
    let _ = fs::remove_dir(releases);
    undo_moves(moved);
    let _ = fs::remove_dir(staging);
}

/// Move a flat OpenJML unpack out of the shared `tools/` root into
/// `tools/openjml/<version>/`.
///
/// This is also what frees the name. On these machines `tools/openjml` is the
/// launcher *file*, so the per-tool directory cannot be created until it moves.
///
/// Every step is a rename within `tools/`, so the ~490 MB release is never
/// copied and a failure part-way is rolled back. Nothing is deleted: the
/// destination cannot already exist, because the gate above requires
/// `tools/openjml` to be a file, which rules out `tools/openjml/<version>/`.
///
/// Returns the release directory, or `None` when there is nothing to migrate.
pub fn migrate_legacy_flat_install() -> io::Result<Option<PathBuf>> {
    migrate_legacy_flat_install_in(&resolve_forall_home().join("tools"))
}

fn migrate_legacy_flat_install_in(tools: &Path) -> io::Result<Option<PathBuf>> {
    let Some(version) = legacy_flat_openjml_version(tools) else {
        return Ok(None);
    };
    if !is_safe_dir_name(&version) {
        return Err(io::Error::other(format!(
            "version-info.txt names an unusable directory: {version:?}"
        )));
    }

    let releases = tools.join("openjml");
    let staging = tools.join(format!(".openjml-migrate-{version}"));
    // Not `remove_dir_all`: anything inside is the release a previous run moved
    // and could not put back, so clearing it recursively would delete exactly
    // what needs recovering. Refuse instead and name the directory.
    let _ = fs::remove_dir(&staging);
    if staging.exists() {
        return Err(io::Error::other(format!(
            "{} already holds files from an interrupted migration; move them back into {} or delete the directory, then retry",
            staging.display(),
            tools.display()
        )));
    }
    fs::create_dir_all(&staging)?;

    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in fs::read_dir(tools)?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_openjml_release_entry(name) {
            continue;
        }
        // Here `openjml` is the launcher file. If it is ever a directory the
        // per-tool root already exists, and moving it would bury real installs.
        if name == "openjml" && !path.is_file() {
            continue;
        }
        let dest = staging.join(name);
        if let Err(err) = fs::rename(&path, &dest) {
            abandon_migration(&moved, &releases, &staging);
            return Err(io::Error::other(format!(
                "could not move {} aside: {err}",
                path.display()
            )));
        }
        moved.push((path, dest));
    }

    if let Err(err) = fs::create_dir_all(&releases) {
        abandon_migration(&moved, &releases, &staging);
        return Err(io::Error::other(format!(
            "could not create {}: {err}",
            releases.display()
        )));
    }
    let release_dir = releases.join(&version);
    if let Err(err) = fs::rename(&staging, &release_dir) {
        abandon_migration(&moved, &releases, &staging);
        return Err(io::Error::other(format!(
            "could not place the migrated install at {}: {err}",
            release_dir.display()
        )));
    }
    Ok(Some(release_dir))
}

/// A release this run unpacked.
struct InstalledOpenjml {
    tag: String,
    release_dir: PathBuf,
    launcher: PathBuf,
}

/// Download the latest OpenJML release and unpack it into
/// `~/.forall/tools/openjml/<tag>/`.
///
/// Unpacked into staging and renamed into place, so an interrupted install
/// cannot leave a half-written release that later looks installed. The archive
/// is checked against the digest the release API serves for it, and the
/// unpacked launcher is then probed, so neither a corrupt download nor a
/// half-unpacked tree can be recorded as an install.
fn install_openjml_release(quiet: bool) -> Result<InstalledOpenjml, String> {
    let fragment = platform_asset_fragment()?;

    let release: GithubRelease = curl_json(RELEASES_API)
        .and_then(|raw| serde_json::from_str(&raw).map_err(io::Error::other))
        .map_err(|err| format!("failed to read OpenJML release metadata: {err}"))?;

    let asset = find_platform_asset(&release.assets).ok_or_else(|| {
        format!(
            "OpenJML release {} has no asset matching {fragment}",
            release.tag_name
        )
    })?;

    if !is_safe_dir_name(&release.tag_name) {
        return Err(format!(
            "OpenJML release tag is not usable as a directory name: {:?}",
            release.tag_name
        ));
    }

    let releases = forall_openjml_releases_root();
    let release_dir = releases.join(&release.tag_name);
    let staging = releases.join(format!(".incoming-{}", release.tag_name));

    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|err| {
        format!(
            "could not create {}: {err} (is {} a file left by an older install?)",
            staging.display(),
            releases.display()
        )
    })?;

    if !quiet {
        eprintln!(
            "forall ensure-openjml: downloading {} ({} MB)",
            asset.name,
            asset.size / 1_000_000
        );
    }

    let archive = staging.join(&asset.name);
    if let Err(err) = curl_download(&asset.browser_download_url, &archive) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("failed to download OpenJML: {err}"));
    }

    // Fail closed, and only hash once a digest is known to exist so a refusal
    // does not cost a needless pass over 164 MB.
    let verdict = match expected_sha256(asset) {
        Some(expected) => digest_verdict(&expected, sha256_file(&archive)),
        None => Err(missing_digest_message(&release.tag_name, &asset.name)),
    };
    if let Err(message) = verdict {
        let _ = fs::remove_dir_all(&staging);
        return Err(message);
    }

    if let Err(err) = unzip_archive(&archive, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("failed to unpack OpenJML: {err}"));
    }
    let _ = fs::remove_file(&archive);

    let Some(staged_launcher) = launcher_in_release_dir(&staging) else {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "OpenJML launcher missing after unpack (expected {})",
            staging.join("openjml").display()
        ));
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&staged_launcher) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = fs::set_permissions(&staged_launcher, perms);
        }
    }

    let _ = fs::remove_dir_all(&release_dir);
    if let Err(err) = fs::rename(&staging, &release_dir) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("could not place the OpenJML install: {err}"));
    }

    let launcher = launcher_in_release_dir(&release_dir).ok_or_else(|| {
        format!(
            "OpenJML launcher missing at {} after install",
            release_dir.display()
        )
    })?;

    Ok(InstalledOpenjml {
        tag: release.tag_name,
        release_dir,
        launcher,
    })
}

fn failure(message: String) -> EnsureOpenjmlResult {
    EnsureOpenjmlResult {
        ok: false,
        already_installed: false,
        installed: false,
        version: None,
        wrapper_path: None,
        install_root: None,
        message,
    }
}

/// Install or verify the OpenJML toolchain under `$FORALL_HOME/tools/openjml/`.
///
/// When `allow_install` is true this links from `FORALL_OPENJML_HOME`
/// candidates and, failing that, downloads the official release — the same
/// lazy provisioning Verus and LemmaScript already do from hooks. `--check-only`
/// turns it off. A local install passed to [`link_openjml_from`] still wins, and
/// is still never copied.
pub fn ensure_openjml_toolchain(allow_install: bool, quiet: bool) -> EnsureOpenjmlResult {
    let initially_available = openjml_version_check().is_some();
    let mut installed_root: Option<PathBuf> = None;
    let mut installed_tag: Option<String> = None;

    // Give a hand-unpacked release the layout an install would get, which is
    // also what frees the `tools/openjml` name for the per-tool root. Renames
    // only, and the gate stops matching afterwards, so this runs at most once.
    if allow_install {
        match migrate_legacy_flat_install() {
            Ok(Some(release_dir)) => {
                if !quiet {
                    eprintln!(
                        "forall ensure-openjml: moved the existing OpenJML release out of the shared tools root into {}",
                        release_dir.display()
                    );
                }
                if let Some(launcher) = launcher_in_release_dir(&release_dir) {
                    let _ = link_openjml_from(&launcher);
                }
            }
            Ok(None) => {}
            Err(err) => {
                if !quiet {
                    eprintln!(
                        "forall ensure-openjml: warning: could not move the existing install into {}: {err}",
                        forall_openjml_releases_root().display()
                    );
                }
            }
        }
    }

    // Discovery accepts a launcher on `is_file()` alone, so a stale or corrupt
    // release would otherwise be found, block the install branch, and fail the
    // probe below — leaving every later run stuck on the same broken file with
    // no way out but manual deletion. Probing here lets a reinstall replace it.
    // An explicit `FORALL_OPENJML` is left alone: it wins at resolve time
    // regardless, so installing over it would not repair anything.
    let discovered = find_openjml_binary();
    let discovered_runs = discovered
        .as_deref()
        .is_some_and(|binary| probe_launcher(binary).is_some());
    let keep_discovered = discovered
        .as_deref()
        .filter(|_| discovered_runs || !allow_install || has_explicit_openjml_override());

    if let Some(binary) = keep_discovered {
        if let Err(err) = write_openjml_wrapper(&forall_openjml_wrapper(), binary)
            && !quiet
        {
            eprintln!("forall ensure-openjml: warning: could not refresh wrapper: {err}");
        }
    } else if allow_install {
        if let Some(broken) = discovered.as_deref()
            && !quiet
        {
            eprintln!(
                "forall ensure-openjml: {} did not run; installing a fresh release",
                broken.display()
            );
        }
        let mut linked = false;
        for candidate in default_openjml_candidates() {
            if resolve_openjml_script(&candidate).is_some() && link_openjml_from(&candidate).is_ok()
            {
                if !quiet {
                    eprintln!(
                        "forall ensure-openjml: linked OpenJML from {}",
                        candidate.display()
                    );
                }
                linked = true;
                break;
            }
        }
        if !linked {
            match install_openjml_release(quiet) {
                Ok(install) => {
                    if let Err(err) = link_openjml_from(&install.launcher) {
                        return failure(format!(
                            "installed OpenJML to {} but could not write the wrapper: {err}",
                            install.release_dir.display()
                        ));
                    }
                    installed_root = Some(install.release_dir);
                    installed_tag = Some(install.tag);
                }
                Err(message) => return failure(message),
            }
        }
    }

    if let Some(version) = openjml_version_check() {
        let message = match (&installed_root, &installed_tag) {
            (Some(root), Some(tag)) => format!("installed OpenJML {tag} to {}", root.display()),
            _ if initially_available => "OpenJML already available".to_string(),
            _ => format!("linked OpenJML at {}", forall_openjml_wrapper().display()),
        };
        return EnsureOpenjmlResult {
            ok: true,
            already_installed: initially_available,
            installed: !initially_available,
            version: Some(version),
            wrapper_path: Some(forall_openjml_wrapper()),
            install_root: installed_root,
            message,
        };
    }

    if let Some(root) = installed_root {
        return failure(format!(
            "installed OpenJML to {} but it did not run",
            root.display()
        ));
    }

    failure(openjml_install_hint())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_openjml_binary_skips_forall_wrapper() {
        let wrapper = forall_openjml_wrapper();
        assert!(is_forall_managed_openjml(&wrapper));
        if let Some(binary) = find_openjml_binary() {
            assert!(
                !is_forall_managed_openjml(&binary),
                "must not resolve to Forall wrapper: {}",
                binary.display()
            );
        }
    }

    #[test]
    fn default_candidates_use_forall_openjml_home_only() {
        let candidates = default_openjml_candidates();
        for path in &candidates {
            let raw = path.display().to_string();
            assert!(
                !raw.contains("Documents/astrio"),
                "should not hardcode developer paths: {raw}"
            );
        }
    }

    #[test]
    fn install_root_does_not_collide_with_openjml_script_path() {
        let root = forall_openjml_install_root();
        let legacy = legacy_openjml_source_marker();
        assert!(root.ends_with("openjml-install"));
        assert_ne!(root, resolve_forall_home().join("tools").join("openjml"));
        assert!(legacy.ends_with("openjml/source.txt"));
    }

    #[test]
    fn releases_root_is_a_per_tool_directory() {
        let releases = forall_openjml_releases_root();
        assert!(releases.ends_with("tools/openjml"));
        // The metadata directory must stay outside it, or removing a release
        // would take the link marker with it.
        assert!(!forall_openjml_install_root().starts_with(&releases));
    }

    /// The macOS/Linux/Windows assets published for 21.0.27. The Linux x86_64
    /// name carries the runner release, which is why matching is by fragment.
    const RELEASE_ASSETS: &[&str] = &[
        "openjml-macos-arm64-21.0.27.zip",
        "openjml-macos-x86_64-21.0.27.zip",
        "openjml-ubuntu-24.04-21.0.27.zip",
        "openjml-ubuntu-arm64-21.0.27.zip",
        "openjml-windows-21.0.27.zip",
    ];

    fn assets() -> Vec<GithubAsset> {
        RELEASE_ASSETS
            .iter()
            .map(|name| GithubAsset {
                name: (*name).to_string(),
                browser_download_url: format!("https://example.invalid/{name}"),
                size: 164_381_279,
                digest: Some(
                    "sha256:265e8459d99ba64499e0ee0b309070182287372ef7f7c0b919dbc37b9dca7ee8"
                        .to_string(),
                ),
            })
            .collect()
    }

    /// Every supported platform picks the asset upstream actually published
    /// for it. Runs on any host, so a macOS build still checks the Linux rows —
    /// `-ubuntu-` prefixes *both* Linux assets, and matching it loosely would
    /// hand an x86_64 machine the arm64 build.
    #[test]
    fn each_platform_selects_the_asset_published_for_it() {
        let all = assets();
        for (os, arch, expected) in [
            ("macos", "aarch64", "openjml-macos-arm64-21.0.27.zip"),
            ("macos", "x86_64", "openjml-macos-x86_64-21.0.27.zip"),
            ("linux", "x86_64", "openjml-ubuntu-24.04-21.0.27.zip"),
            ("linux", "aarch64", "openjml-ubuntu-arm64-21.0.27.zip"),
        ] {
            let fragment = asset_fragment_for(os, arch)
                .unwrap_or_else(|err| panic!("{os} {arch} should be supported: {err}"));
            let picked = select_asset(&all, fragment)
                .unwrap_or_else(|| panic!("no asset selected for {os} {arch}"));
            assert_eq!(picked.name, expected, "wrong asset for {os} {arch}");
        }
    }

    #[test]
    fn windows_is_refused_because_the_wrapper_is_a_bash_script() {
        // Upstream publishes openjml-windows-<tag>.zip, so the message must not
        // claim otherwise — the limitation is Forall's wrapper, and blaming
        // upstream would send the user looking for a build that exists.
        assert!(
            RELEASE_ASSETS.iter().any(|name| name.contains("-windows-")),
            "upstream still publishes a Windows asset"
        );

        let err = asset_fragment_for("windows", "x86_64").expect_err("windows must be refused");
        assert!(
            !err.contains("does not publish"),
            "must not blame upstream for a build it publishes: {err}"
        );
        assert!(
            err.contains("bash script"),
            "must name the real cause: {err}"
        );
        assert!(
            err.contains("--path"),
            "must offer the linking route instead: {err}"
        );
        // arm64 Windows takes the same route rather than the generic message.
        assert!(asset_fragment_for("windows", "aarch64").is_err());
    }

    #[test]
    fn an_unpublished_platform_says_so() {
        let err = asset_fragment_for("freebsd", "x86_64").expect_err("no freebsd build");
        assert!(err.contains("does not publish"), "{err}");
    }

    #[test]
    fn asset_digest_is_read_from_the_release_api_field() {
        // The real digest the API served for openjml-macos-arm64-21.0.27.zip,
        // which matches shasum of the downloaded archive.
        let asset = &assets()[0];
        assert_eq!(
            expected_sha256(asset).as_deref(),
            Some("265e8459d99ba64499e0ee0b309070182287372ef7f7c0b919dbc37b9dca7ee8")
        );

        let mut malformed = assets().remove(0);
        malformed.digest = Some("sha512:abc".to_string());
        assert_eq!(expected_sha256(&malformed), None);

        malformed.digest = Some("sha256:nothex".to_string());
        assert_eq!(expected_sha256(&malformed), None);

        // Older releases predate the field; those install without a digest
        // check rather than failing.
        malformed.digest = None;
        assert_eq!(expected_sha256(&malformed), None);
    }

    #[test]
    fn an_archive_that_cannot_be_verified_is_not_unpacked() {
        let digest = "265e8459d99ba64499e0ee0b309070182287372ef7f7c0b919dbc37b9dca7ee8";
        assert!(digest_verdict(digest, Ok(digest.to_string())).is_ok());

        // A mismatch is refused.
        let err = digest_verdict(digest, Ok("0".repeat(64))).expect_err("mismatch must fail");
        assert!(err.contains("checksum mismatch"), "{err}");

        // So is being unable to compute one. The archive gets unpacked and
        // executed, so an unverifiable download must not install anyway.
        let err = digest_verdict(
            digest,
            Err(io::Error::other("sha256 checksum command failed")),
        )
        .expect_err("an unverifiable download must fail");
        assert!(err.contains("cannot verify"), "{err}");
        assert!(
            err.contains("--path"),
            "must offer the linking route: {err}"
        );

        // And so is a release that announces no digest at all.
        let err = missing_digest_message("21.0.27", "openjml-macos-arm64-21.0.27.zip");
        assert!(err.contains("no sha256 digest"), "{err}");
        assert!(err.contains("--path"), "{err}");
    }

    #[test]
    fn a_launcher_that_cannot_run_is_not_treated_as_installed() {
        let dir = tempfile::tempdir().unwrap();

        // A file that exists but is not executable — a half-unpacked release,
        // or one that lost its execute bit. Discovery accepts it on is_file();
        // the probe is what tells it apart from a working install.
        let broken = dir.path().join("openjml");
        fs::write(&broken, "not a program\n").unwrap();
        assert_eq!(launcher_in_release_dir(dir.path()), Some(broken.clone()));
        assert_eq!(probe_launcher(&broken), None);

        // A launcher that answers `-?` is accepted.
        let working = dir.path().join("works");
        fs::write(&working, "#!/bin/sh\necho 'Usage: openjml <options>'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&working).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&working, perms).unwrap();
        }
        assert!(probe_launcher(&working).is_some());
    }

    #[test]
    fn an_interrupted_migration_is_not_cleared_out_from_under_the_user() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        write_flat_release(&tools, "21.0.27");

        // What a run killed mid-move leaves behind: part of the release sitting
        // in staging. Clearing that recursively would delete the very files
        // that need recovering.
        let staging = tools.join(".openjml-migrate-21.0.27");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("jmlruntime.jar"), "rescued\n").unwrap();

        let err = migrate_legacy_flat_install_in(&tools).expect_err("must refuse, not clobber");
        assert!(
            err.to_string().contains("interrupted migration"),
            "must name the situation: {err}"
        );
        assert_eq!(
            fs::read_to_string(staging.join("jmlruntime.jar")).unwrap(),
            "rescued\n",
            "the stranded file must still be there"
        );
    }

    #[test]
    fn abandoning_a_migration_frees_the_name_before_restoring_the_launcher() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        let staging = tools.join(".staging");
        let releases = tools.join("openjml");
        fs::create_dir_all(&staging).unwrap();
        // The half-built per-tool root occupying the launcher's own path.
        fs::create_dir_all(&releases).unwrap();
        let launcher = tools.join("openjml");
        let stashed = staging.join("openjml");
        fs::write(&stashed, "#!/usr/bin/env bash\n").unwrap();

        abandon_migration(&[(launcher.clone(), stashed)], &releases, &staging);

        // The launcher is back where it started, not deleted with the staging
        // directory, and nothing of the release was left stranded.
        assert!(launcher.is_file(), "launcher must be restored");
        assert_eq!(
            fs::read_to_string(&launcher).unwrap(),
            "#!/usr/bin/env bash\n"
        );
        assert!(!staging.exists(), "empty staging is cleaned up");
    }

    #[test]
    fn directory_names_from_upstream_cannot_escape_the_tool_root() {
        assert!(is_safe_dir_name("21.0.27"));
        assert!(is_safe_dir_name("v1.2.3-rc1"));
        assert!(!is_safe_dir_name(""));
        assert!(!is_safe_dir_name(".."));
        assert!(!is_safe_dir_name("../../etc"));
        assert!(!is_safe_dir_name("21.0.27/openjml"));
    }

    /// Reproduces the flat layout the release archive actually unpacks into:
    /// 29 top-level entries with no directory of their own.
    fn write_flat_release(tools: &Path, version: &str) {
        fs::create_dir_all(tools.join("jdk/bin")).unwrap();
        fs::create_dir_all(tools.join("specs")).unwrap();
        fs::create_dir_all(tools.join("demos")).unwrap();
        fs::write(tools.join("openjml"), "#!/usr/bin/env bash\n").unwrap();
        fs::write(tools.join("setup-vars"), "# sourced\n").unwrap();
        fs::write(
            tools.join("version-info.txt"),
            format!("{version}\nOpenJML\n"),
        )
        .unwrap();
        fs::write(tools.join("README"), "openjml\n").unwrap();
        fs::write(tools.join("jdk/bin/java"), "binary\n").unwrap();
    }

    #[test]
    fn migration_moves_a_flat_unpack_under_the_per_tool_root() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        write_flat_release(&tools, "21.0.27");
        // A neighbouring toolchain, and Forall's own link metadata, must stay.
        fs::create_dir_all(tools.join("lemmascript/node_modules")).unwrap();
        fs::create_dir_all(tools.join("openjml-install")).unwrap();
        fs::write(tools.join("openjml-install/source.txt"), "/somewhere\n").unwrap();

        let release = migrate_legacy_flat_install_in(&tools)
            .unwrap()
            .expect("a flat install to migrate");
        assert_eq!(release, tools.join("openjml").join("21.0.27"));

        // The release moved wholesale, launcher included.
        assert!(release.join("openjml").is_file());
        assert!(release.join("jdk/bin/java").is_file());
        assert!(release.join("specs").is_dir());
        assert!(release.join("version-info.txt").is_file());
        assert_eq!(
            launcher_in_release_dir(&release),
            Some(release.join("openjml"))
        );

        // The shared root is no longer littered with the release's entries.
        assert!(!tools.join("jdk").exists());
        assert!(!tools.join("specs").exists());
        assert!(!tools.join("README").exists());
        assert!(!tools.join("setup-vars").exists());

        // Neighbours are untouched.
        assert!(tools.join("lemmascript/node_modules").is_dir());
        assert!(tools.join("openjml-install/source.txt").is_file());
    }

    #[test]
    fn migration_leaves_entries_it_does_not_recognise_where_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        write_flat_release(&tools, "21.0.27");
        // A tool root added after this code was written, and a stray file.
        fs::create_dir_all(tools.join("some-future-prover/bin")).unwrap();
        fs::write(tools.join("notes.txt"), "mine\n").unwrap();

        let release = migrate_legacy_flat_install_in(&tools).unwrap().unwrap();

        // The release moved; the unrecognised entries did not.
        assert!(release.join("openjml").is_file());
        assert!(tools.join("some-future-prover/bin").is_dir());
        assert!(tools.join("notes.txt").is_file());
        assert!(!release.join("some-future-prover").exists());
        assert!(!release.join("notes.txt").exists());
    }

    #[test]
    fn the_link_metadata_directory_is_not_mistaken_for_release_content() {
        // It shares the `openjml` prefix with the launcher family.
        assert!(!is_openjml_release_entry("openjml-install"));
        assert!(is_openjml_release_entry("openjml"));
        assert!(is_openjml_release_entry("openjml-java"));
        assert!(is_openjml_release_entry("openjml.vsix"));
        assert!(is_openjml_release_entry("Solvers-macos-arm64"));
        assert!(is_openjml_release_entry("jdk"));
        assert!(!is_openjml_release_entry("lemmascript"));
        assert!(!is_openjml_release_entry("verus"));
    }

    #[test]
    fn migration_is_idempotent_and_leaves_a_clean_install_alone() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        write_flat_release(&tools, "21.0.27");

        assert!(migrate_legacy_flat_install_in(&tools).unwrap().is_some());
        // Second run finds `tools/openjml` is a directory, so the gate no
        // longer matches and nothing moves.
        assert_eq!(migrate_legacy_flat_install_in(&tools).unwrap(), None);
        assert!(tools.join("openjml/21.0.27/openjml").is_file());
    }

    #[test]
    fn a_tools_root_without_a_flat_unpack_is_not_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let tools = dir.path().join("tools");
        fs::create_dir_all(tools.join("lemmascript")).unwrap();
        // A bare `openjml` file with none of the release's siblings is not a
        // release, so it must not be swept up.
        fs::write(tools.join("openjml"), "#!/bin/sh\n").unwrap();

        assert_eq!(migrate_legacy_flat_install_in(&tools).unwrap(), None);
        assert!(tools.join("openjml").is_file());
        assert!(tools.join("lemmascript").is_dir());
    }

    #[test]
    fn launcher_is_found_flat_and_one_level_down() {
        let dir = tempfile::tempdir().unwrap();
        let flat = dir.path().join("flat");
        fs::create_dir_all(&flat).unwrap();
        fs::write(flat.join("openjml"), "#!/bin/sh\n").unwrap();
        assert_eq!(launcher_in_release_dir(&flat), Some(flat.join("openjml")));

        let nested = dir.path().join("nested");
        fs::create_dir_all(nested.join("OpenJMLsrc")).unwrap();
        fs::write(nested.join("OpenJMLsrc/openjml"), "#!/bin/sh\n").unwrap();
        assert_eq!(
            launcher_in_release_dir(&nested),
            Some(nested.join("OpenJMLsrc/openjml"))
        );

        let empty = dir.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(launcher_in_release_dir(&empty), None);
    }

    #[test]
    fn detect_java_source_paths_finds_src() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/logic")).unwrap();
        std::fs::write(root.join("src/logic/Foo.java"), "class Foo {}\n").unwrap();
        let paths = detect_java_source_paths(root);
        assert_eq!(paths, vec![root.join("src")]);
        let arg = java_sourcepath_arg(root).expect("sourcepath");
        assert!(arg.contains("src"));
    }
}
