use std::path::Path;

use anyhow::Result;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(forall_utils_cargo_bin::cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

#[test]
fn debug_models_bundled_prints_json() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut cmd = forall_command(forall_home.path())?;
    let output = cmd.args(["debug", "models", "--bundled"]).output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    assert!(value["models"].is_array());
    assert!(!value["models"].as_array().unwrap_or(&Vec::new()).is_empty());

    Ok(())
}

#[test]
fn debug_models_default_prints_json_without_auth() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut cmd = forall_command(forall_home.path())?;
    let output = cmd.args(["debug", "models"]).output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    let value: serde_json::Value = serde_json::from_str(&stdout)?;
    assert!(value["models"].is_array());
    assert!(!value["models"].as_array().unwrap_or(&Vec::new()).is_empty());

    Ok(())
}
