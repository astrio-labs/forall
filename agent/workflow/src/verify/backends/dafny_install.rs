//! Provisioning and detection for Dafny, the solver behind LemmaScript.
//!
//! LemmaScript compiles `//@` annotations to Dafny and shells out to a bare
//! `dafny` on PATH. Nothing installed it and nothing checked for it, so on a
//! machine without Dafny a project whose contracts are correct reported
//!
//!     CRITICAL proofs: proof check failed for src/pricing.ts
//!     INFO     proofs: LemmaScript/Dafny proof obligations failed.
//!                      Fix //@ contracts and .dfy proofs
//!
//! telling the user to repair contracts that prove cleanly the moment Dafny is
//! present. Every other backend has an availability check, an installer and a
//! hint; this module gives Dafny the same three.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::toolchain_download::{check_asset_digest, staging_dir_for, sweep_abandoned_staging};
use crate::agent_home::resolve_forall_home;

/// Stable releases only. Dafny publishes nightlies to the same repository —
/// twenty-odd `dafny-nightly-<date>-<sha>-…` assets were live when this was
/// written — and `/releases/latest` is the endpoint that excludes them.
const RELEASES_API: &str = "https://api.github.com/repos/dafny-lang/dafny/releases/latest";

#[derive(Debug, Clone)]
pub struct EnsureDafnyResult {
    pub ok: bool,
    pub already_installed: bool,
    pub installed: bool,
    pub version: Option<String>,
    pub binary_path: Option<PathBuf>,
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
    /// `sha256:<hex>`, stamped by GitHub on every release asset. Dafny's
    /// release notes are a changelog and carry no hash, so this is the only
    /// checksum upstream publishes — see [`super::toolchain_download`].
    #[serde(default)]
    digest: Option<String>,
}

pub fn dafny_install_hint() -> String {
    "Dafny not found. Run `forall ensure-dafny` to install the official release, or set FORALL_DAFNY to a binary named `dafny`.".to_string()
}

pub fn forall_dafny_install_root() -> PathBuf {
    resolve_forall_home().join("tools").join("dafny")
}

/// The archive unpacks to a single top-level `dafny/` directory holding the
/// launcher and a bundled Z3, so the binary is always `<release>/dafny/dafny`.
/// Unlike Verus, that directory name carries no version or platform, so there
/// is nothing to keep in sync with the asset name.
fn binary_in_release_dir(release_dir: &Path) -> PathBuf {
    let name = if cfg!(windows) { "dafny.exe" } else { "dafny" };
    release_dir.join("dafny").join(name)
}

/// An explicit override wins over anything Forall installed.
///
/// The file name has to be `dafny`. LemmaScript shells out to a bare `dafny`,
/// and the only way to point it at an override is to prepend that override's
/// directory to the child's PATH, which reaches it only if the name matches.
/// Accepting `dafny-4.11` would report the prover as available and then hand
/// it work it could never receive — the false green this module exists to
/// prevent.
fn configured_dafny() -> Option<PathBuf> {
    let raw = std::env::var("FORALL_DAFNY").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.file_stem().and_then(|name| name.to_str()) != Some("dafny") {
        return None;
    }
    Some(path)
}

/// Every Dafny binary Forall has installed, newest release directory last.
fn installed_dafny_binaries() -> Vec<PathBuf> {
    let root = forall_dafny_install_root();
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let candidate = binary_in_release_dir(&path);
        if candidate.is_file() {
            let stamp = fs::metadata(&candidate)
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            found.push((stamp, candidate));
        }
    }
    found.sort_by_key(|(stamp, _)| *stamp);
    found.into_iter().map(|(_, path)| path).collect()
}

/// Directory to prepend to `PATH` so a child process finds the Dafny that
/// answered the availability probe.
///
/// This is what makes a non-PATH install reachable at all. LemmaScript invokes
/// a bare `dafny` with no override, so putting the binary under `$FORALL_HOME`
/// — or naming it in `FORALL_DAFNY` — is not enough on its own; see
/// `lemmascript::run_lsc`.
///
/// It has to be the same binary [`dafny_version_check`] found, which is why
/// both go through [`resolve_dafny`]. Scanning the managed install root
/// separately meant an override passed the probe and was then never exposed,
/// so `lsc` fell back to a bare `dafny` that did not exist, exited 0, and
/// Forall recorded a proof that never ran.
pub fn forall_dafny_bin_dir() -> Option<PathBuf> {
    resolve_dafny()?
        .binary
        .and_then(|binary| binary.parent().map(Path::to_path_buf))
}

/// Run a candidate and return its reported version.
///
/// A liveness probe rather than a file check: a half-unpacked release must
/// report unavailable rather than be handed work it cannot do.
fn probe_version(binary: &Path) -> Option<String> {
    let mut command = Command::new(binary);
    command.arg("--version");
    crate::verify::command_security::apply_hosted_identity(&mut command);
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

/// The Dafny that will answer, and the version it reported.
#[derive(Clone, Debug)]
struct ResolvedDafny {
    /// `None` when a bare `dafny` already resolves on PATH, so there is
    /// nothing for [`forall_dafny_bin_dir`] to prepend.
    binary: Option<PathBuf>,
    version: String,
}

/// Cached because `run_lsc` asks for the PATH entry on every invocation and
/// each miss costs a `dafny --version` process start. Cleared after an
/// install, since a check run probes once before installing and again after.
static RESOLVED: std::sync::Mutex<Option<Option<ResolvedDafny>>> = std::sync::Mutex::new(None);

/// The first candidate that answers, in the order a proof would reach them.
fn probe_dafny() -> Option<ResolvedDafny> {
    let mut candidates = Vec::new();
    if let Some(configured) = configured_dafny() {
        candidates.push(configured);
    }
    candidates.extend(installed_dafny_binaries().into_iter().rev());
    for candidate in candidates {
        if let Some(version) = probe_version(&candidate) {
            return Some(ResolvedDafny {
                binary: Some(candidate),
                version,
            });
        }
    }
    probe_version(Path::new("dafny")).map(|version| ResolvedDafny {
        binary: None,
        version,
    })
}

fn resolve_dafny() -> Option<ResolvedDafny> {
    let mut cache = match RESOLVED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(cached) = cache.as_ref() {
        return cached.clone();
    }
    let resolved = probe_dafny();
    *cache = Some(resolved.clone());
    resolved
}

/// Drop the cached probe so the next caller looks again.
fn forget_resolved_dafny() {
    let mut cache = match RESOLVED.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *cache = None;
}

/// The version of whichever Dafny would actually be used, or `None`.
pub fn dafny_version_check() -> Option<String> {
    resolve_dafny().map(|resolved| resolved.version)
}

pub fn dafny_is_available() -> bool {
    resolve_dafny().is_some()
}

/// The `<arch>-<os>` fragment of the asset name for this platform.
///
/// Matched as a fragment rather than a suffix on purpose. Dafny stamps the
/// runner's OS release into the name, and that moves: `arm64-macos-11`,
/// `arm64-macos-13`, `x64-ubuntu-20.04`, `x64-ubuntu-22.04`,
/// `x64-windows-2019`, `x64-windows-2022` have all shipped. Pinning a whole
/// suffix would break on the next runner upgrade.
fn platform_fragment() -> Result<&'static str, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("-arm64-macos"),
        ("macos", "x86_64") => Ok("-x64-macos"),
        ("linux", "x86_64") => Ok("-x64-ubuntu"),
        ("windows", "x86_64") => Ok("-x64-windows"),
        // Dafny has never published an arm64 Linux build. Saying so is better
        // than reporting a missing asset, which reads like a packaging gap
        // worth retrying.
        ("linux", "aarch64") => Err(
            "Dafny does not publish a prebuilt toolchain for arm64 Linux. Install Dafny from source or a distribution package, then set FORALL_DAFNY to the resulting `dafny` binary."
                .to_string(),
        ),
        (os, arch) => Err(format!(
            "Dafny does not publish a prebuilt toolchain for {os} {arch}. Set FORALL_DAFNY to a binary named `dafny`."
        )),
    }
}

fn find_platform_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    let fragment = platform_fragment().ok()?;
    assets
        .iter()
        .find(|asset| asset.name.ends_with(".zip") && asset.name.contains(fragment))
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
            "curl failed downloading Dafny asset: {}",
            String::from_utf8_lossy(&output.stderr).trim()
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
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

fn failure(message: String) -> EnsureDafnyResult {
    EnsureDafnyResult {
        ok: false,
        already_installed: false,
        installed: false,
        version: None,
        binary_path: None,
        message,
    }
}

/// Install or verify Dafny under `$FORALL_HOME/tools/dafny/`.
///
/// The release archive is self-contained: it bundles its own .NET runtime and
/// two Z3 binaries, so nothing else has to be present for it to run.
pub fn ensure_dafny_toolchain(allow_download: bool, quiet: bool) -> EnsureDafnyResult {
    if let Some(version) = dafny_version_check() {
        return EnsureDafnyResult {
            ok: true,
            already_installed: true,
            installed: false,
            version: Some(version),
            binary_path: installed_dafny_binaries().last().cloned(),
            message: "Dafny already available".to_string(),
        };
    }

    let fragment = match platform_fragment() {
        Ok(fragment) => fragment,
        Err(message) => return failure(message),
    };

    if !allow_download {
        return failure(dafny_install_hint());
    }

    let release: GithubRelease = match curl_json(RELEASES_API)
        .and_then(|raw| serde_json::from_str(&raw).map_err(io::Error::other))
    {
        Ok(release) => release,
        Err(err) => return failure(format!("failed to read Dafny release metadata: {err}")),
    };

    let Some(asset) = find_platform_asset(&release.assets) else {
        return failure(format!(
            "Dafny release {} has no asset matching {fragment}",
            release.tag_name
        ));
    };

    let install_root = forall_dafny_install_root();
    let release_dir = install_root.join(&release.tag_name);
    sweep_abandoned_staging(&release_dir);
    let staging = staging_dir_for(&release_dir);

    // Unpack into a staging directory and rename into place, so a killed or
    // failed install cannot leave a half-written release that later looks
    // installed.
    let _ = fs::remove_dir_all(&staging);
    if let Err(err) = fs::create_dir_all(&staging) {
        return failure(format!("could not create {}: {err}", staging.display()));
    }

    let archive = staging.join(&asset.name);
    if let Err(err) = curl_download(&asset.browser_download_url, &archive) {
        let _ = fs::remove_dir_all(&staging);
        return failure(format!("failed to download Dafny: {err}"));
    }

    // Nothing checked these bytes before. Dafny's release notes are a
    // changelog with no hash in them, so the digest GitHub stamps on the asset
    // is the only checksum upstream publishes.
    let digest_check = check_asset_digest(&archive, asset.digest.as_deref());
    if digest_check.is_fatal() {
        let _ = fs::remove_dir_all(&staging);
        return failure(format!("Dafny {}", digest_check.describe()));
    }
    if !quiet {
        eprintln!("forall ensure-dafny: {}", digest_check.describe());
    }

    if let Err(err) = unzip_archive(&archive, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return failure(format!("failed to unpack Dafny: {err}"));
    }
    let _ = fs::remove_file(&archive);

    let staged_binary = binary_in_release_dir(&staging);
    if !staged_binary.is_file() {
        let _ = fs::remove_dir_all(&staging);
        return failure(format!(
            "Dafny binary missing after unpack (expected {})",
            staged_binary.display()
        ));
    }

    // Another process may have published this same release already; its tree
    // passed the same digest check, so keep it rather than delete a good
    // install to replace it with an identical one.
    if binary_in_release_dir(&release_dir).is_file() {
        let _ = fs::remove_dir_all(&staging);
    } else {
        // Only a partial or stale tree can be here, and rename will not land
        // on a non-empty directory.
        let _ = fs::remove_dir_all(&release_dir);
        if let Err(err) = fs::rename(&staging, &release_dir) {
            let _ = fs::remove_dir_all(&staging);
            return failure(format!("could not place Dafny install: {err}"));
        }
    }
    // A check run probes before installing and again after, so the "not
    // found" answer from the first probe must not outlive this.
    forget_resolved_dafny();

    let binary = binary_in_release_dir(&release_dir);
    let Some(version) = probe_version(&binary) else {
        return failure(format!(
            "installed Dafny at {} but it did not run",
            binary.display()
        ));
    };

    if !quiet {
        eprintln!(
            "forall ensure-dafny: installed {} to {}",
            release.tag_name,
            release_dir.display()
        );
    }

    EnsureDafnyResult {
        ok: true,
        already_installed: false,
        installed: true,
        version: Some(version),
        binary_path: Some(binary),
        message: format!(
            "installed Dafny {} to {}",
            release.tag_name,
            release_dir.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
            digest: None,
        }
    }

    /// The digest is the only checksum Dafny publishes — the release notes
    /// are a changelog with no hash in them — and nothing verified the
    /// download before. Deserializing proves the field this depends on is the
    /// shape GitHub sends.
    #[test]
    fn the_asset_carries_the_published_digest() {
        let parsed: GithubAsset = serde_json::from_str(
            r#"{
                "name": "dafny-4.11.0-arm64-macos-13.zip",
                "browser_download_url": "https://example.invalid/dafny.zip",
                "digest": "sha256:c90c75e7d5db9c6ccbb7127840dfe43f0ac938b039a7ebed146d8ead383a572f"
            }"#,
        )
        .expect("asset");
        assert!(
            parsed
                .digest
                .as_deref()
                .is_some_and(|digest| digest.starts_with("sha256:"))
        );
        // A release without the field must still install rather than fail to
        // parse; the absence is reported instead.
        assert_eq!(asset("dafny.zip").digest, None);
    }

    /// Dafny stamps the CI runner's OS release into the asset name, and that
    /// moves between releases. Matching a whole suffix would break on the next
    /// runner upgrade, which is the failure mode that left Verus unable to
    /// install on any platform.
    #[test]
    fn the_asset_is_matched_by_fragment_so_os_versions_can_drift() {
        let assets = vec![
            asset("dafny-4.11.0-arm64-macos-13.zip"),
            asset("dafny-4.11.0-x64-macos-13.zip"),
            asset("dafny-4.11.0-x64-ubuntu-22.04.zip"),
            asset("dafny-4.11.0-x64-windows-2022.zip"),
            asset("DafnyRef.pdf"),
        ];
        let older = vec![
            asset("dafny-4.9.0-arm64-macos-11.zip"),
            asset("dafny-4.9.0-x64-ubuntu-20.04.zip"),
        ];

        // Whatever this test runs on, the fragment must select a zip and never
        // the PDF.
        if let Ok(fragment) = platform_fragment() {
            let picked = find_platform_asset(&assets).map(|a| a.name.as_str());
            if let Some(name) = picked {
                assert!(name.contains(fragment), "{name} does not carry {fragment}");
                assert!(name.ends_with(".zip"));
            }
            // The same rule still matches an older release with a different
            // OS version stamped in.
            if find_platform_asset(&assets).is_some()
                && older.iter().any(|a| a.name.contains(fragment))
            {
                assert!(find_platform_asset(&older).is_some());
            }
        }
    }

    #[test]
    fn a_platform_without_a_prebuilt_release_says_so_plainly() {
        // The message must name the situation rather than a missing asset, so
        // a user does not retry forever waiting for a build that will not come.
        if let Err(message) = platform_fragment() {
            assert!(message.contains("does not publish"), "{message}");
            assert!(message.contains("FORALL_DAFNY"), "{message}");
        }
    }

    #[test]
    fn the_binary_sits_under_a_stable_directory_name() {
        let dir = tempfile::tempdir().unwrap();
        let expected = if cfg!(windows) { "dafny.exe" } else { "dafny" };
        assert_eq!(
            binary_in_release_dir(dir.path()),
            dir.path().join("dafny").join(expected)
        );
    }

    /// An override is exposed to LemmaScript by prepending its directory, and
    /// LemmaScript asks for a bare `dafny`. A binary under any other name can
    /// never be reached that way, so accepting it would report the prover as
    /// available and then let `lsc` exit 0 without proving anything — the
    /// false green this module exists to prevent.
    #[test]
    fn an_override_must_be_named_dafny_to_be_reachable() {
        let dir = tempfile::tempdir().unwrap();
        let reachable = dir.path().join("dafny");
        let renamed = dir.path().join("dafny-4.11");

        assert_eq!(
            reachable.file_stem().and_then(|name| name.to_str()),
            Some("dafny"),
            "the directory of this path does expose a bare `dafny`"
        );
        assert_ne!(
            renamed.file_stem().and_then(|name| name.to_str()),
            Some("dafny"),
            "prepending this path's directory would not make `dafny` resolve"
        );
    }

    /// The PATH entry has to name the same binary the probe accepted.
    /// Resolving them separately let an override satisfy the probe while the
    /// managed install root supplied the PATH entry, so `lsc` ran a Dafny
    /// nobody had checked, or none at all.
    #[test]
    fn the_path_entry_is_the_directory_of_the_resolved_binary() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = ResolvedDafny {
            binary: Some(dir.path().join("dafny")),
            version: "Dafny 4.11.0".to_string(),
        };
        assert_eq!(
            resolved
                .binary
                .and_then(|bin| bin.parent().map(Path::to_path_buf)),
            Some(dir.path().to_path_buf())
        );

        // A Dafny already on PATH needs nothing prepended.
        let on_path = ResolvedDafny {
            binary: None,
            version: "Dafny 4.11.0".to_string(),
        };
        assert_eq!(on_path.binary, None);
    }
}
