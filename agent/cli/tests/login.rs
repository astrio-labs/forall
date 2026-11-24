use std::path::Path;

use anyhow::Result;
use forall_utils_cargo_bin::cargo_bin;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

fn write_file_auth_config(forall_home: &Path) -> Result<()> {
    std::fs::write(
        forall_home.join("config.toml"),
        "cli_auth_credentials_store = \"file\"\n",
    )?;
    Ok(())
}

fn read_auth_json(forall_home: &Path) -> Result<Value> {
    let auth_json = std::fs::read_to_string(forall_home.join("auth.json"))?;
    Ok(serde_json::from_str(&auth_json)?)
}

#[test]
fn login_with_api_key_reads_stdin_and_writes_auth_json() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_file_auth_config(forall_home.path())?;

    let mut cmd = forall_command(forall_home.path())?;
    cmd.args([
        "-c",
        "forced_login_method=\"api\"",
        "login",
        "--with-api-key",
    ])
    .write_stdin("sk-test\n")
    .assert()
    .success()
    .stderr(contains("Successfully saved OpenAI API key"));

    let auth = read_auth_json(forall_home.path())?;
    assert_eq!(auth["OPENAI_API_KEY"], "sk-test");
    assert!(auth.get("tokens").is_none());

    Ok(())
}

#[test]
fn login_without_flags_prints_byok_guidance() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_file_auth_config(forall_home.path())?;

    forall_command(forall_home.path())?
        .args(["login"])
        .assert()
        .failure()
        .stderr(contains("bring-your-own-key"));

    Ok(())
}
