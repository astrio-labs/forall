//! Integrity checking for downloaded toolchain archives.
//!
//! Forall downloads provers and then trusts them to produce proofs, so a
//! truncated or substituted archive is not a cosmetic problem. The check that
//! existed read the expected hash out of the GitHub release *notes*:
//!
//! ```text
//! if let Some(expected) = parse_expected_sha256(&release.body, asset_name) {
//! ```
//!
//! Neither upstream writes one there. Measured against the live API, the whole
//! body of the current Verus release is 36 characters —
//! `release/rolling/0.2026.08.02.b677dd5` — and Dafny's is a changelog with no
//! hash anywhere in it. So the parse always returned `None`, the block was
//! skipped in silence, and every install went unverified while the code read
//! as though it were checking.
//!
//! GitHub itself stamps every release asset with a digest, returned as
//! `sha256:<hex>` alongside the download URL. That is an actual value on an
//! actual field, present for both projects, and it is what gets checked here.
//!
//! What this buys is integrity of transfer — a truncated download, a
//! mangling proxy, a corrupted cache. The digest arrives from the same host as
//! the bytes, so it is not a defence against GitHub itself, and this module
//! does not pretend otherwise.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A staging directory beside `release_dir`, to be renamed onto it once the
/// unpacked tree is known to be complete.
///
/// Beside it, so the rename stays within one filesystem — the system temp
/// directory offers no such guarantee and `rename` fails across mounts.
///
/// Named from the final path component only, because release tags are not
/// always a single segment. Verus publishes `release/0.2026.08.02.b677dd5`,
/// and interpolating that whole string into a directory name builds a nested
/// path whose outer half survives the rename as an empty directory.
///
/// The process id is on the end because installs are not serialised. Forall
/// acquires toolchains from hooks, so two runs in two repositories can reach
/// the same release at the same time. Sharing one staging directory meant each
/// began by deleting it, so one run would remove the other's half-downloaded
/// archive or its unpacked tree, and both would fail somewhere confusing.
pub fn staging_dir_for(release_dir: &Path) -> PathBuf {
    let leaf = release_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "release".to_string());
    release_dir.with_file_name(format!(".incoming-{leaf}-{}", std::process::id()))
}

/// How long an untouched staging directory has to sit before a later install
/// treats it as abandoned. Generous on purpose: a slow download on a poor
/// connection must never have its own staging swept out from under it.
const ABANDONED_STAGING: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Remove staging directories left behind by installs that never finished.
///
/// Staging is per-process, so nothing else will ever clean ours up and a
/// killed install would otherwise strand tens of megabytes indefinitely. Only
/// age decides: a directory belonging to a live install is minutes old at
/// most, and one belonging to a dead one is never touched again.
pub fn sweep_abandoned_staging(release_dir: &Path) {
    let Some(parent) = release_dir.parent() else {
        return;
    };
    let Some(leaf) = release_dir.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let prefix = format!(".incoming-{leaf}-");
    let mine = staging_dir_for(release_dir);
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == mine || !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(&prefix))
        {
            continue;
        }
        let abandoned = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
            .is_ok_and(|age| age > ABANDONED_STAGING);
        if abandoned {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// What an integrity check concluded, so a caller can report it rather than
/// decide it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestCheck {
    /// The archive matched the digest GitHub published for it.
    Verified,
    /// There was nothing to check against.
    ///
    /// Not a failure. A missing digest is not evidence of a bad download, and
    /// refusing to install would strand a user over a field upstream chose not
    /// to send. It must still be said out loud: the whole defect this replaces
    /// was an unverified download that looked verified.
    NoDigestPublished(String),
    /// The archive did not match. Always fatal — the bytes are wrong.
    Mismatch { expected: String, actual: String },
}

impl DigestCheck {
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }

    /// A line fit for the install output and for the returned message.
    pub fn describe(&self) -> String {
        match self {
            Self::Verified => "sha256 verified against the published digest".to_string(),
            Self::NoDigestPublished(reason) => {
                format!("download NOT verified: {reason}")
            }
            Self::Mismatch { expected, actual } => {
                format!("download checksum mismatch (expected {expected}, got {actual})")
            }
        }
    }
}

/// Check a downloaded archive against the digest GitHub published for it.
///
/// `digest` is the asset's `digest` field, of the form `sha256:<hex>`.
pub fn check_asset_digest(path: &Path, digest: Option<&str>) -> DigestCheck {
    let Some(digest) = digest.map(str::trim).filter(|value| !value.is_empty()) else {
        return DigestCheck::NoDigestPublished(
            "GitHub published no digest for this asset".to_string(),
        );
    };
    let Some(expected) = digest.strip_prefix("sha256:") else {
        // A digest in an algorithm we cannot compute is not a mismatch. Saying
        // "wrong bytes" about bytes never compared would be its own lie.
        return DigestCheck::NoDigestPublished(format!(
            "published digest is not sha256 ({digest})"
        ));
    };
    let expected = expected.trim().to_ascii_lowercase();
    match sha256_file(path) {
        Ok(actual) if actual == expected => DigestCheck::Verified,
        Ok(actual) => DigestCheck::Mismatch { expected, actual },
        Err(err) => DigestCheck::NoDigestPublished(format!("could not compute sha256: {err}")),
    }
}

/// The sha256 of a file, lowercase hex.
///
/// Shells out because the workspace has no hashing dependency in this crate
/// and every supported platform ships one of these two.
pub fn sha256_file(path: &Path) -> io::Result<String> {
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
    let hash = line
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(io::Error::other(format!(
            "sha256 command produced no usable digest: {}",
            line.trim()
        )));
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn file_with(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("asset.zip");
        fs::write(&path, contents).expect("write");
        (dir, path)
    }

    #[test]
    fn a_matching_digest_verifies() {
        let (_guard, path) = file_with("forall");
        let actual = sha256_file(&path).expect("sha256");
        assert_eq!(
            check_asset_digest(&path, Some(&format!("sha256:{actual}"))),
            DigestCheck::Verified
        );
        // GitHub's casing must not decide the outcome.
        assert_eq!(
            check_asset_digest(&path, Some(&format!("sha256:{}", actual.to_uppercase()))),
            DigestCheck::Verified
        );
    }

    /// The bytes are wrong. Nothing else this module reports is fatal, and
    /// this always is.
    #[test]
    fn a_wrong_digest_is_fatal() {
        let (_guard, path) = file_with("forall");
        let check = check_asset_digest(&path, Some(&format!("sha256:{}", "0".repeat(64))));
        assert!(check.is_fatal(), "{check:?}");
        assert!(check.describe().contains("mismatch"), "{check:?}");
    }

    /// The defect being replaced: no digest meant the check was skipped in
    /// silence, so an unverified download was indistinguishable from a
    /// verified one. It stays non-fatal, but it can no longer stay quiet.
    #[test]
    fn a_missing_digest_is_reported_rather_than_skipped() {
        let (_guard, path) = file_with("forall");
        for absent in [None, Some(""), Some("   ")] {
            let check = check_asset_digest(&path, absent);
            assert!(!check.is_fatal(), "{check:?}");
            assert!(
                check.describe().contains("NOT verified"),
                "the caller has to be able to say so: {check:?}"
            );
        }
    }

    /// An algorithm we cannot compute is not evidence of wrong bytes, so it
    /// must not be reported as a mismatch.
    #[test]
    fn an_unknown_algorithm_is_unverified_not_a_mismatch() {
        let (_guard, path) = file_with("forall");
        let check = check_asset_digest(&path, Some("sha512:abc"));
        assert!(!check.is_fatal(), "{check:?}");
        assert!(check.describe().contains("not sha256"), "{check:?}");
    }

    /// Two installs of the same release must not share a staging directory:
    /// each one begins by deleting it, so one run would remove the other's
    /// half-downloaded archive or its unpacked tree.
    #[test]
    fn staging_is_private_to_the_process() {
        let release = Path::new("/home/tools/verus/v1.0.0");
        let staging = staging_dir_for(release);
        let name = staging.file_name().and_then(|n| n.to_str()).unwrap_or("");
        assert!(name.starts_with(".incoming-v1.0.0-"), "{name}");
        assert!(
            name.ends_with(&std::process::id().to_string()),
            "another process must not compute this same path: {name}"
        );
        assert_eq!(staging.parent(), release.parent());
    }

    /// Per-process staging means nothing else will ever clean ours up, so a
    /// killed install would strand its download forever. Only age decides,
    /// and generously: a live install's staging is minutes old at most.
    #[test]
    fn only_long_abandoned_staging_is_swept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let release = dir.path().join("v1.0.0");
        fs::create_dir_all(&release).expect("release");

        let fresh = dir.path().join(".incoming-v1.0.0-999999");
        fs::create_dir_all(&fresh).expect("fresh");
        let unrelated = dir.path().join(".incoming-v2.0.0-999999");
        fs::create_dir_all(&unrelated).expect("unrelated");
        let mine = staging_dir_for(&release);
        fs::create_dir_all(&mine).expect("mine");

        sweep_abandoned_staging(&release);

        assert!(fresh.is_dir(), "a staging directory in use must survive");
        assert!(mine.is_dir(), "this process's own staging must survive");
        assert!(unrelated.is_dir(), "another release's staging is not ours to remove");
        assert!(release.is_dir(), "the published release is never touched");
    }

    /// Verus tags carry a slash — `release/0.2026.08.02.b677dd5` — so naming
    /// the staging directory after the whole tag built a nested path and left
    /// its outer half behind as an empty directory after the rename. Measured
    /// on a real install before this was taken from the leaf.
    #[test]
    fn staging_sits_beside_the_release_directory_whatever_the_tag() {
        let root = Path::new("/home/tools/verus");

        let simple = staging_dir_for(&root.join("v4.11.0"));
        assert_eq!(
            simple.parent(),
            Some(root),
            "same parent, so the rename stays on one filesystem"
        );
        assert!(
            simple
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".incoming-v4.11.0-")),
            "{simple:?}"
        );

        let slashed = root.join("release/0.2026.08.02.b677dd5");
        let staging = staging_dir_for(&slashed);
        assert!(
            staging
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(".incoming-0.2026.08.02.b677dd5-")),
            "{staging:?}"
        );
        assert_eq!(
            staging.parent(),
            slashed.parent(),
            "a multi-segment tag must not push staging into a directory of its own"
        );
    }
}
