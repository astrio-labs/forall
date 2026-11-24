//! Ensures `target/forall-path/rg` exists for local `cargo` builds so sandboxed
//! exec inherits ripgrep the same way standalone package installs do.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const RIPGREP_VERSION: &str = "14.1.1";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-ObjC");
    }

    if env::var_os("FORALL_SKIP_DEV_RG").is_some() {
        println!("cargo:rerun-if-env-changed=FORALL_SKIP_DEV_RG");
        return;
    }

    if let Err(err) = ensure_dev_ripgrep() {
        println!("cargo:warning=dev ripgrep bootstrap skipped: {err}");
    }
}

fn ensure_dev_ripgrep() -> io::Result<()> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|_| {
            manifest_dir
                .join("../target")
                .canonicalize()
                .map_err(|err| io::Error::new(io::ErrorKind::NotFound, err))
        })
        .unwrap_or_else(|_| manifest_dir.join("../target"));

    let path_dir = target_dir.join("forall-path");
    let rg_name = if cfg!(windows) { "rg.exe" } else { "rg" };
    let rg_path = path_dir.join(rg_name);

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FORALL_SKIP_DEV_RG");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");

    if rg_path.is_file() && rg_ready(&rg_path) {
        return Ok(());
    }

    fs::create_dir_all(&path_dir)?;

    if copy_host_rg(&rg_path)? {
        return Ok(());
    }

    download_ripgrep(&path_dir, &rg_path)
}

fn rg_ready(path: &Path) -> bool {
    Command::new(path)
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn copy_host_rg(dest: &Path) -> io::Result<bool> {
    let Some(host) = host_rg_path() else {
        return Ok(false);
    };
    if !host.is_file() {
        return Ok(false);
    }
    let _ = fs::remove_file(dest);
    fs::copy(&host, dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(dest, perms)?;
    }
    Ok(rg_ready(dest))
}

fn host_rg_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let output = Command::new("sh")
            .args(["-c", "command -v rg"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?;
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        let candidate = PathBuf::from(path);
        candidate.is_file().then_some(candidate)
    }
    #[cfg(windows)]
    {
        let output = Command::new("where").arg("rg").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8(output.stdout).ok()?;
        let path = path.lines().next()?.trim();
        if path.is_empty() {
            return None;
        }
        let candidate = PathBuf::from(path);
        candidate.is_file().then_some(candidate)
    }
}

fn download_ripgrep(path_dir: &Path, rg_path: &Path) -> io::Result<()> {
    let Some(triple) = ripgrep_target_triple() else {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no ripgrep release for this target; install rg on the host",
        ));
    };

    let archive_name = if cfg!(windows) {
        format!("ripgrep-{RIPGREP_VERSION}-{triple}.zip")
    } else {
        format!("ripgrep-{RIPGREP_VERSION}-{triple}.tar.gz")
    };
    let url = format!(
        "https://github.com/BurntSushi/ripgrep/releases/download/{RIPGREP_VERSION}/{archive_name}"
    );

    let scratch = path_dir.join(".ripgrep-download");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch)?;
    let archive_path = scratch.join(&archive_name);

    let status = Command::new("curl")
        .args(["-fsSL", &url, "-o"])
        .arg(&archive_path)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("curl failed to fetch {url}")));
    }

    let extracted_rg = {
        #[cfg(windows)]
        {
            extract_archive_zip(&archive_path, &scratch)?
        }
        #[cfg(not(windows))]
        {
            extract_tar_gz(&archive_path, &scratch)?
        }
    };

    let _ = fs::remove_file(rg_path);
    fs::copy(&extracted_rg, rg_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(rg_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(rg_path, perms)?;
    }
    let _ = fs::remove_dir_all(&scratch);
    Ok(())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> io::Result<PathBuf> {
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()?;
    if !status.success() {
        return Err(io::Error::other("tar extraction failed"));
    }
    find_extracted_rg(dest)
}

#[cfg(windows)]
fn extract_archive_zip(archive: &Path, dest: &Path) -> io::Result<PathBuf> {
    let status = Command::new("tar")
        .args(["-xf"])
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()?;
    if !status.success() {
        return Err(io::Error::other("zip extraction failed"));
    }
    find_extracted_rg(dest)
}

fn find_extracted_rg(root: &Path) -> io::Result<PathBuf> {
    let rg_name = if cfg!(windows) { "rg.exe" } else { "rg" };
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let candidate = path.join(rg_name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "ripgrep binary not found in extracted archive",
    ))
}

fn ripgrep_target_triple() -> Option<&'static str> {
    let os = env::var("CARGO_CFG_TARGET_OS").ok()?;
    let arch = env::var("CARGO_CFG_TARGET_ARCH").ok()?;
    match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}
