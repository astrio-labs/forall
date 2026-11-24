//! `FORALL_DAFNY` has to reach LemmaScript, not just satisfy the probe.
//!
//! LemmaScript shells out to a bare `dafny`, so the only way an override is
//! ever used is if Forall prepends its directory to the child's PATH. The
//! availability probe honoured the override while the PATH entry was resolved
//! separately from the managed install root, so setting `FORALL_DAFNY` made
//! Forall report the prover as present and then hand `lsc` a `dafny` that did
//! not exist. `lsc` exits 0 in that case, so the result was a recorded proof
//! that never ran.
//!
//! This sets a process-global environment variable and therefore lives in its
//! own test binary, where no other test can observe it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(unix)]
#[test]
fn a_configured_dafny_is_exposed_on_the_child_path() {
    use forall_workflow::verify::backends::dafny_install::{
        dafny_version_check, forall_dafny_bin_dir,
    };
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let binary = dir.path().join("dafny");
    // The probe runs the candidate, so the stub has to answer `--version`.
    std::fs::write(&binary, "#!/bin/sh\necho 'Dafny 9.9.9'\n").expect("write stub");
    let mut perms = std::fs::metadata(&binary).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&binary, perms).expect("chmod");

    // SAFETY: single-threaded, and this binary contains one test.
    unsafe {
        std::env::set_var("FORALL_DAFNY", &binary);
    }

    assert_eq!(
        dafny_version_check().as_deref(),
        Some("Dafny 9.9.9"),
        "the override must be the Dafny that answers"
    );
    assert_eq!(
        forall_dafny_bin_dir().as_deref(),
        Some(dir.path()),
        "and its directory must be the one prepended to the child's PATH"
    );
}
