use anyhow::Result;
use predicates::str::contains;
use std::path::Path;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(forall_utils_cargo_bin::cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

#[cfg(debug_assertions)]
#[tokio::test]
async fn update_does_not_start_interactive_prompt() -> Result<()> {
    let forall_home = TempDir::new()?;

    forall_command(forall_home.path())?
        .arg("update")
        .assert()
        .failure()
        .stderr(contains("`forall update` is not available in debug builds"));

    Ok(())
}
