use std::process::Command;

use anyhow::Result;
use tempfile::TempDir;

#[test]
fn strict_config_rejects_unknown_config_fields_for_standalone_app_server() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"
foo = "bar"
"#,
    )?;

    let output = Command::new(forall_utils_cargo_bin::cargo_bin("forall-app-server")?)
        .env("FORALL_HOME", forall_home.path())
        .env(
            "CODEX_APP_SERVER_MANAGED_CONFIG_PATH",
            forall_home.path().join("managed_config.toml"),
        )
        .args(["--strict-config", "--listen", "off"])
        .output()?;

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert!(
        stderr.contains("unknown configuration field `foo`"),
        "expected strict config error in stderr, got: {stderr}"
    );

    Ok(())
}
