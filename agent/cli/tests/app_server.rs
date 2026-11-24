use std::path::Path;

use anyhow::Result;
use predicates::str::contains;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(forall_utils_cargo_bin::cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

#[test]
fn strict_config_rejects_unknown_config_fields_for_app_server() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"
foo = "bar"
"#,
    )?;

    let mut cmd = forall_command(forall_home.path())?;
    cmd.args(["app-server", "--strict-config", "--listen", "off"])
        .assert()
        .failure()
        .stderr(contains("unknown configuration field"));

    Ok(())
}
