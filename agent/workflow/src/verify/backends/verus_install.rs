use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::toolchain_download::{check_asset_digest, staging_dir_for, sweep_abandoned_staging};
use crate::agent_home::resolve_forall_home;

/// An install that got far enough to fail rather than never start.
fn failed_install(message: String) -> EnsureVerusResult {
    EnsureVerusResult {
        ok: false,
        already_installed: false,
        installed: false,
        version: None,
        toolchain_installed: false,
        wrapper_path: None,
        message,
    }
}

const RELEASES_API: &str = "https://api.github.com/repos/verus-lang/verus/releases/latest";

#[derive(Debug, Clone)]
pub struct EnsureVerusResult {
    pub ok: bool,
    pub already_installed: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub toolchain_installed: bool,
    pub wrapper_path: Option<PathBuf>,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, stamped by GitHub on every release asset. The release
    /// notes carry no hash — see [`super::toolchain_download`] — so this is
    /// the only checksum upstream actually publishes.
    #[serde(default)]
    digest: Option<String>,
}

pub fn verus_install_hint() -> String {
    "Verus not found. Run `forall ensure-verus` to install the official release, or set FORALL_VERUS to a custom command.".to_string()
}

pub fn forall_cargo_verus_wrapper() -> PathBuf {
    resolve_forall_home().join("bin").join("cargo-verus")
}

pub fn verus_is_available() -> bool {
    verus_version_check().is_some()
}

pub fn verus_version_check() -> Option<String> {
    let wrapper = forall_cargo_verus_wrapper();
    if wrapper.is_file()
        && let Ok(out) = Command::new(&wrapper).arg("--version").output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }

    if let Ok(out) = Command::new("cargo").args(["verus", "--version"]).output()
        && out.status.success()
    {
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

/// Zip filename suffixes for this platform (newest naming first, then legacy).
///
/// Recent Verus releases publish versioned assets such as
/// `verus-0.2026.07.27.31579f0-arm64-macos.zip`. Older releases used unversioned
/// names like `verus-arm64-macos.zip`.
fn platform_asset_suffixes() -> Option<&'static [&'static str]> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(&["-arm64-macos.zip"]),
        ("macos", "x86_64") => Some(&["-x86-macos.zip", "-x64-macos.zip"]),
        ("linux", "x86_64") => Some(&["-x86-linux.zip", "-x64-linux.zip"]),
        // ARM Linux is deliberately absent: upstream has never published an
        // aarch64/arm64 Linux asset in any release, and INSTALL.md lists the
        // prebuilt matrix as MacOS arm, MacOS x86_64, Windows x86_64 and
        // Ubuntu x86_64. Naming one produced a "release has no asset" error
        // that read like a transient packaging gap worth retrying, when the
        // only route is building from source.
        //
        // Windows x86_64 IS published (`-x86-win.zip`) and is omitted only
        // because the wrapper below is a bash script written under
        // cfg(unix); wiring it up means more than a match arm.
        _ => None,
    }
}

fn find_platform_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    let suffixes = platform_asset_suffixes()?;
    for suffix in suffixes {
        if let Some(asset) = assets.iter().find(|a| a.name.ends_with(suffix)) {
            return Some(asset);
        }
    }
    None
}

fn platform_dir_candidates() -> Option<&'static [&'static str]> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some(&["verus-arm64-macos"]),
        ("macos", "x86_64") => Some(&["verus-x86-macos", "verus-x64-macos"]),
        ("linux", "x86_64") => Some(&["verus-x86-linux", "verus-x64-linux"]),
        _ => None,
    }
}

fn curl_json(url: &str) -> io::Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: forall-ensure-verus",
            url,
        ])
        .output()
        .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl failed fetching release metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
            "curl failed downloading Verus asset: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
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
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn find_verus_binary(install_dir: &Path) -> Option<PathBuf> {
    let candidates = platform_dir_candidates()?;
    for dir in candidates {
        let candidate = install_dir.join(dir).join("verus");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Verus binaries under `$FORALL_HOME/tools/verus/`.
///
/// The install layout is `tools/verus/<release-tag>/<platform-dir>/verus`, so
/// this looks exactly one level down for the release tag and no further. The
/// original walked the whole tree recursively and followed symlinks, which on
/// a populated home means descending a 100MB+ toolchain on every call.
fn collect_installed_verus_binaries(root: &Path, out: &mut Vec<PathBuf>) {
    if let Some(bin) = find_verus_binary(root) {
        out.push(bin);
    }
    let Ok(read_dir) = fs::read_dir(root) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        // Symlinks are not followed: a link back up the tree would recurse
        // without bound, and no release layout needs one.
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(bin) = find_verus_binary(&path)
        {
            out.push(bin);
        }
    }
}

/// Newest Verus binary under `$FORALL_HOME/tools/verus/` (by mtime), if any.
fn find_newest_installed_verus_binary() -> Option<PathBuf> {
    let root = resolve_forall_home().join("tools").join("verus");
    if !root.is_dir() {
        return None;
    }
    let mut bins = Vec::new();
    collect_installed_verus_binaries(&root, &mut bins);
    bins.into_iter().max_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    })
}

/// Upgrade a wrapper written by an older Forall, in place.
///
/// Wrappers written before cargo-verus delegation drove the raw `verus`
/// binary over a `find`-and-`grep` file list, which loses crate context. This
/// rewrites those.
///
/// It runs on the ALREADY-INSTALLED path, which `ensure_verus_toolchain`
/// reaches from the PreToolUse hook — so on every application-code edit. It
/// must therefore do nothing at all in the common case. Reading one small
/// file and comparing a marker is the whole cost when the wrapper is current;
/// the directory scan only happens when it is genuinely stale.
fn refresh_cargo_verus_wrapper_from_forall_install() -> io::Result<()> {
    let wrapper = forall_cargo_verus_wrapper();
    if wrapper_delegates_to_cargo_verus(&wrapper) {
        return Ok(());
    }
    let Some(verus_bin) = find_newest_installed_verus_binary() else {
        return Ok(());
    };
    write_cargo_verus_wrapper(&wrapper, &verus_bin)
}

/// Whether an existing wrapper is the current, cargo-verus-delegating shape.
///
/// A wrapper written before that delegation drove the raw `verus` binary over
/// a `find`-and-`grep` file list, which loses crate context. Absent or
/// unreadable counts as not-current, so it gets rewritten.
fn wrapper_delegates_to_cargo_verus(wrapper: &Path) -> bool {
    fs::read_to_string(wrapper).is_ok_and(|body| body.contains("CARGO_VERUS="))
}

fn expected_verus_binary(install_dir: &Path) -> Option<PathBuf> {
    platform_dir_candidates()?
        .first()
        .map(|dir| install_dir.join(dir).join("verus"))
}

fn write_cargo_verus_wrapper(wrapper_path: &Path, verus_bin: &Path) -> io::Result<()> {
    if let Some(parent) = wrapper_path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Require the official cargo-verus next to `verus` so `cargo verus verify`
    // keeps full crate context. Never fall back to the raw `verus` binary — that
    // accepts different args and breaks multi-file crates.
    let cargo_verus = verus_bin
        .parent()
        .map(|dir| dir.join("cargo-verus"))
        .filter(|p| p.is_file())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Verus install is missing cargo-verus next to {} (required for crate-aware verify); re-run `forall ensure-verus`",
                    verus_bin.display()
                ),
            )
        })?;
    let script = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
VERUS_BIN={verus_bin:?}
CARGO_VERUS={cargo_verus:?}
case "${{1:-}}" in
  --version|-V)
    exec "$VERUS_BIN" --version
    ;;
  *)
    exec "$CARGO_VERUS" "$@"
    ;;
esac
"#,
        verus_bin = verus_bin.display(),
        cargo_verus = cargo_verus.display()
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

fn parse_toolchain_from_version(version_output: &str) -> Option<String> {
    for line in version_output.lines() {
        // Strip ANSI color codes Verus may emit on stderr.
        let trimmed = strip_ansi(line).trim().to_string();
        if let Some(rest) = trimmed.strip_prefix("Toolchain:") {
            let toolchain = rest.trim();
            if !toolchain.is_empty() {
                return Some(toolchain.to_string());
            }
        }
        // `verus --version` fails before printing Toolchain: when rustup is missing:
        //   verus: required rust toolchain 1.97.1-aarch64-apple-darwin not found
        if let Some(rest) = trimmed
            .strip_prefix("verus: required rust toolchain ")
            .and_then(|s| s.strip_suffix(" not found"))
        {
            let toolchain = rest.trim();
            if !toolchain.is_empty() {
                return Some(toolchain.to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("rustup install ") {
            let toolchain = rest.trim();
            if !toolchain.is_empty() {
                return Some(toolchain.to_string());
            }
        }
        if let Some(rest) = trimmed.strip_prefix("rustup toolchain install ") {
            let toolchain = rest.trim();
            if !toolchain.is_empty() {
                return Some(toolchain.to_string());
            }
        }
    }
    None
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn parse_toolchain_from_version_json(verus_bin: &Path) -> Option<String> {
    let json_path = verus_bin.parent()?.join("version.json");
    let raw = fs::read_to_string(json_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .pointer("/verus/toolchain")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn ensure_rustup_toolchain(toolchain: &str) -> io::Result<bool> {
    let status = Command::new("rustup")
        .args(["toolchain", "install", toolchain])
        .status()
        .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "rustup toolchain install {toolchain} failed"
        )));
    }
    Ok(true)
}

/// Install or verify the Verus toolchain under `$FORALL_HOME/tools/verus/`.
pub fn ensure_verus_toolchain(allow_download: bool, quiet: bool) -> EnsureVerusResult {
    if let Some(version) = verus_version_check() {
        if let Err(err) = refresh_cargo_verus_wrapper_from_forall_install()
            && !quiet
        {
            eprintln!("forall ensure-verus: warning: failed to refresh cargo-verus wrapper: {err}");
        }
        return EnsureVerusResult {
            ok: true,
            already_installed: true,
            installed: false,
            version: Some(version),
            toolchain_installed: false,
            wrapper_path: Some(forall_cargo_verus_wrapper()),
            message: "Verus already available".to_string(),
        };
    }

    if !allow_download {
        return EnsureVerusResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            toolchain_installed: false,
            wrapper_path: None,
            message: verus_install_hint(),
        };
    }

    if platform_asset_suffixes().is_none() || platform_dir_candidates().is_none() {
        return EnsureVerusResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            toolchain_installed: false,
            wrapper_path: None,
            message: format!(
                "unsupported platform for automatic Verus install: {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        };
    }

    let release_json = match curl_json(RELEASES_API) {
        Ok(raw) => raw,
        Err(err) => {
            return EnsureVerusResult {
                ok: false,
                already_installed: false,
                installed: false,
                version: None,
                toolchain_installed: false,
                wrapper_path: None,
                message: format!("failed to fetch Verus release metadata: {err}"),
            };
        }
    };

    let release: GithubRelease = match serde_json::from_str(&release_json) {
        Ok(release) => release,
        Err(err) => {
            return EnsureVerusResult {
                ok: false,
                already_installed: false,
                installed: false,
                version: None,
                toolchain_installed: false,
                wrapper_path: None,
                message: format!("failed to parse Verus release metadata: {err}"),
            };
        }
    };

    let Some(asset) = find_platform_asset(&release.assets) else {
        let suffixes = platform_asset_suffixes().unwrap_or(&[]);
        return EnsureVerusResult {
            ok: false,
            already_installed: false,
            installed: false,
            version: None,
            toolchain_installed: false,
            wrapper_path: None,
            message: format!(
                "release {} has no asset matching suffixes {:?}",
                release.tag_name, suffixes
            ),
        };
    };
    let asset_name = asset.name.as_str();

    let forall_home = resolve_forall_home();
    let install_root = forall_home
        .join("tools")
        .join("verus")
        .join(&release.tag_name);

    let verus_bin = if let Some(existing) = find_verus_binary(&install_root) {
        if let Err(err) = write_cargo_verus_wrapper(&forall_cargo_verus_wrapper(), &existing) {
            return EnsureVerusResult {
                ok: false,
                already_installed: false,
                installed: false,
                version: None,
                toolchain_installed: false,
                wrapper_path: None,
                message: format!("failed to write cargo-verus wrapper: {err}"),
            };
        }
        existing
    } else {
        // Download and unpack into a staging directory beside the release
        // directory, then rename. Unzipping straight into the release
        // directory meant an interrupted install left a partial tree that the
        // next run found and treated as installed, and a killed download left
        // the archive there too. Staging as a sibling also keeps the rename on
        // one filesystem, which the system temp directory does not guarantee.
        sweep_abandoned_staging(&install_root);
        let staging = staging_dir_for(&install_root);
        let _ = fs::remove_dir_all(&staging);
        if let Err(err) = fs::create_dir_all(&staging) {
            return failed_install(format!("could not create {}: {err}", staging.display()));
        }
        let zip_path = staging.join(asset_name);

        if let Err(err) = curl_download(&asset.browser_download_url, &zip_path) {
            let _ = fs::remove_dir_all(&staging);
            return failed_install(format!("failed to download Verus: {err}"));
        }

        let digest_check = check_asset_digest(&zip_path, asset.digest.as_deref());
        if digest_check.is_fatal() {
            let _ = fs::remove_dir_all(&staging);
            return failed_install(format!("Verus {}", digest_check.describe()));
        }
        if !quiet {
            eprintln!("forall ensure-verus: {}", digest_check.describe());
        }

        if let Err(err) = unzip_archive(&zip_path, &staging) {
            let _ = fs::remove_dir_all(&staging);
            return failed_install(format!("failed to unpack Verus: {err}"));
        }
        let _ = fs::remove_file(&zip_path);

        // Nothing is published until the tree has the binary it is supposed to
        // have, so a release directory can never exist in a half-unpacked
        // state.
        if find_verus_binary(&staging).is_none() {
            let expected = expected_verus_binary(&staging)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| staging.join("verus").display().to_string());
            let _ = fs::remove_dir_all(&staging);
            return failed_install(format!("Verus binary missing after unpack (expected {expected})"));
        }

        // Another process may have published this same release while this one
        // was downloading. Its tree passed the same digest check, so take it
        // rather than delete it: removing a good install to replace it with an
        // identical one leaves a window where a third process — or a proof
        // already running out of that directory — sees nothing there at all.
        if find_verus_binary(&install_root).is_some() {
            let _ = fs::remove_dir_all(&staging);
        } else {
            let _ = fs::remove_dir_all(&install_root);
            if let Err(err) = fs::rename(&staging, &install_root) {
                let _ = fs::remove_dir_all(&staging);
                return failed_install(format!("could not place Verus install: {err}"));
            }
        }

        let Some(bin) = find_verus_binary(&install_root) else {
            return failed_install(format!(
                "Verus binary missing after install (expected under {})",
                install_root.display()
            ));
        };

        if let Err(err) = write_cargo_verus_wrapper(&forall_cargo_verus_wrapper(), &bin) {
            return failed_install(format!("failed to write cargo-verus wrapper: {err}"));
        }

        bin
    };

    // Prefer version.json (works even when `verus --version` fails due to a missing
    // rustup toolchain). Fall back to parsing stdout/stderr from --version.
    let mut toolchain = parse_toolchain_from_version_json(&verus_bin);
    let version_output = Command::new(&verus_bin).arg("--version").output().ok();
    let combined_version_text = version_output.as_ref().map(|out| {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    if toolchain.is_none()
        && let Some(ref text) = combined_version_text
    {
        toolchain = parse_toolchain_from_version(text);
    }

    let mut toolchain_installed = false;
    if let Some(ref tc) = toolchain {
        match ensure_rustup_toolchain(tc) {
            Ok(true) => toolchain_installed = true,
            Ok(false) => {}
            Err(_) => {
                if !quiet {
                    eprintln!(
                        "forall ensure-verus: warning: install rustup toolchain {tc} manually"
                    );
                }
            }
        }
    }

    let version = Command::new(&verus_bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    let wrapper = forall_cargo_verus_wrapper();
    let available = verus_version_check().is_some();
    EnsureVerusResult {
        ok: available,
        already_installed: false,
        installed: true,
        version,
        toolchain_installed,
        wrapper_path: Some(wrapper),
        message: if available {
            format!(
                "installed Verus {} to {}",
                release.tag_name,
                install_root.display()
            )
        } else if let Some(tc) = toolchain {
            format!(
                "installed Verus files but version check failed — install rustup toolchain {tc}"
            )
        } else {
            "installed Verus files but version check failed — verify rustup toolchain".to_string()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest arrives on the asset, not in the release notes. Upstream
    /// leaves the notes empty — the whole body of a Verus release is its own
    /// tag — so a hash parsed from there was always absent and the check
    /// never ran. Deserializing proves the field this now depends on is the
    /// shape GitHub sends.
    #[test]
    fn the_asset_carries_the_published_digest() {
        let asset: GithubAsset = serde_json::from_str(
            r#"{
                "name": "verus-0.2026.08.02.b677dd5-arm64-macos.zip",
                "browser_download_url": "https://example.invalid/verus.zip",
                "digest": "sha256:6096ac60a4445669ac497a9c20e74f9d0af1d4edd931c7392c9cdf0b1ae16665"
            }"#,
        )
        .expect("asset");
        assert_eq!(
            asset.digest.as_deref(),
            Some("sha256:6096ac60a4445669ac497a9c20e74f9d0af1d4edd931c7392c9cdf0b1ae16665")
        );

        // An asset without the field must still deserialize: a missing digest
        // is reported, not a parse error that blocks the install outright.
        let older: GithubAsset = serde_json::from_str(
            r#"{"name": "verus.zip", "browser_download_url": "https://example.invalid/v.zip"}"#,
        )
        .expect("asset without digest");
        assert_eq!(older.digest, None);
    }

    #[test]
    fn find_platform_asset_matches_versioned_names() {
        let versioned = "verus-0.2026.07.27.31579f0-arm64-macos.zip";
        let legacy = "verus-arm64-macos.zip";
        assert!(versioned.ends_with("-arm64-macos.zip"));
        assert!(legacy.ends_with("-arm64-macos.zip"));

        let assets = vec![
            GithubAsset {
                name: "placeholder".to_string(),
                browser_download_url: "https://example.com/placeholder".to_string(),
                digest: None,
            },
            GithubAsset {
                name: versioned.to_string(),
                browser_download_url: "https://example.com/arm".to_string(),
                digest: None,
            },
        ];
        // On macOS aarch64 hosts this selects the versioned asset; elsewhere suffixes differ.
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            let asset = find_platform_asset(&assets).expect("macOS arm64 asset");
            assert_eq!(asset.name, versioned);
        }
    }

    #[test]
    fn parse_toolchain_line() {
        let text = "Verus\nVersion: 0.1\nToolchain: 1.96.0-aarch64-apple-darwin\n";
        assert_eq!(
            parse_toolchain_from_version(text),
            Some("1.96.0-aarch64-apple-darwin".to_string())
        );
    }

    #[test]
    fn parse_toolchain_from_missing_rustup_stderr() {
        let text = "\u{1b}[31mverus: required rust toolchain 1.97.1-aarch64-apple-darwin not found\u{1b}[0m\n  \u{1b}[37mrustup install 1.97.1-aarch64-apple-darwin\u{1b}[0m\n";
        assert_eq!(
            parse_toolchain_from_version(text),
            Some("1.97.1-aarch64-apple-darwin".to_string())
        );
    }

    #[test]
    fn parse_toolchain_from_version_json_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bin = dir.path().join("verus");
        fs::write(&bin, b"").expect("bin");
        fs::write(
            dir.path().join("version.json"),
            r#"{"verus":{"toolchain":"1.97.1-aarch64-apple-darwin"}}"#,
        )
        .expect("json");
        assert_eq!(
            parse_toolchain_from_version_json(&bin),
            Some("1.97.1-aarch64-apple-darwin".to_string())
        );
    }

    /// The refresh runs on the already-installed path, which the PreToolUse
    /// hook reaches on every application-code edit. It must recognise a
    /// current wrapper and do nothing, rather than rewriting a shared file.
    #[test]
    fn a_current_wrapper_is_left_alone_and_an_old_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let wrapper = dir.path().join("cargo-verus");

        // Absent: not current, so it would be written.
        assert!(!wrapper_delegates_to_cargo_verus(&wrapper));

        // The pre-delegation shape, which drove `verus` over a find/grep list.
        fs::write(
            &wrapper,
            "#!/usr/bin/env bash\nVERUS_BIN=/x/verus\nexec \"$VERUS_BIN\" \"$@\"\n",
        )
        .unwrap();
        assert!(!wrapper_delegates_to_cargo_verus(&wrapper));

        // The current shape.
        fs::write(
            &wrapper,
            "#!/usr/bin/env bash\nVERUS_BIN=/x/verus\nCARGO_VERUS=/x/cargo-verus\n",
        )
        .unwrap();
        assert!(wrapper_delegates_to_cargo_verus(&wrapper));
    }

    #[test]
    fn write_wrapper_requires_adjacent_cargo_verus() {
        let dir = tempfile::tempdir().expect("tempdir");
        let verus_bin = dir.path().join("verus");
        let wrapper = dir.path().join("bin").join("cargo-verus");
        fs::write(&verus_bin, b"").expect("verus");

        let err = write_cargo_verus_wrapper(&wrapper, &verus_bin).expect_err("missing cargo-verus");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("missing cargo-verus"));

        let cargo_verus = dir.path().join("cargo-verus");
        fs::write(&cargo_verus, b"").expect("cargo-verus");
        write_cargo_verus_wrapper(&wrapper, &verus_bin).expect("wrapper");
        let script = fs::read_to_string(&wrapper).expect("read wrapper");
        assert!(script.contains("CARGO_VERUS="));
        assert!(script.contains(cargo_verus.to_str().expect("utf8")));
        assert!(!script.contains(&format!("CARGO_VERUS={:?}", verus_bin.display())));
    }
}
