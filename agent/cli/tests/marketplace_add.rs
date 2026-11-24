use anyhow::Result;
use forall_config::CONFIG_TOML_FILE;
use forall_core_plugins::installed_marketplaces::marketplace_install_root;
use forall_utils_absolute_path::AbsolutePathBuf;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(forall_utils_cargo_bin::cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

fn write_marketplace_source(source: &Path, marker: &str) -> Result<()> {
    std::fs::create_dir_all(source.join(".agents/plugins"))?;
    std::fs::create_dir_all(source.join("plugins/sample/.forall-plugin"))?;
    std::fs::write(
        source.join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "debug",
  "plugins": [
    {
      "name": "sample",
      "source": {
        "source": "local",
        "path": "./plugins/sample"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        source.join("plugins/sample/.forall-plugin/plugin.json"),
        r#"{"name":"sample"}"#,
    )?;
    std::fs::write(source.join("plugins/sample/marker.txt"), marker)?;
    Ok(())
}

#[tokio::test]
async fn marketplace_add_local_directory_source() -> Result<()> {
    let forall_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_marketplace_source(source.path(), "local ref")?;
    let source_parent = source.path().parent().unwrap();
    let source_arg = format!("./{}", source.path().file_name().unwrap().to_string_lossy());

    forall_command(forall_home.path())?
        .current_dir(source_parent)
        .args(["plugin", "marketplace", "add", source_arg.as_str()])
        .assert()
        .success();

    let installed_root = marketplace_install_root(forall_home.path()).join("debug");
    assert!(!installed_root.exists());

    let config = std::fs::read_to_string(forall_home.path().join(CONFIG_TOML_FILE))?;
    let config: toml::Value = toml::from_str(&config)?;
    let expected_source = source.path().canonicalize()?.display().to_string();
    assert_eq!(
        config["marketplaces"]["debug"]["source_type"].as_str(),
        Some("local")
    );
    assert_eq!(
        config["marketplaces"]["debug"]["source"].as_str(),
        Some(expected_source.as_str())
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_add_json_prints_add_outcome() -> Result<()> {
    let forall_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_marketplace_source(source.path(), "local ref")?;
    let source_parent = source.path().parent().unwrap();
    let source_arg = format!("./{}", source.path().file_name().unwrap().to_string_lossy());

    let assert = forall_command(forall_home.path())?
        .current_dir(source_parent)
        .args([
            "plugin",
            "marketplace",
            "add",
            source_arg.as_str(),
            "--json",
        ])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;
    let expected_installed_root = AbsolutePathBuf::try_from(source.path().canonicalize()?)?;

    assert_eq!(
        actual,
        json!({
            "marketplaceName": "debug",
            "installedRoot": expected_installed_root.as_path().display().to_string(),
            "alreadyAdded": false,
        })
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_add_rejects_local_manifest_file_source() -> Result<()> {
    let forall_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_marketplace_source(source.path(), "local ref")?;
    let manifest_path = source.path().join(".agents/plugins/marketplace.json");

    forall_command(forall_home.path())?
        .args([
            "plugin",
            "marketplace",
            "add",
            manifest_path.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains(
            "local marketplace source must be a directory, not a file",
        ));

    Ok(())
}

#[tokio::test]
async fn marketplace_add_rejects_sparse_for_local_directory_source() -> Result<()> {
    let forall_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_marketplace_source(source.path(), "local ref")?;

    forall_command(forall_home.path())?
        .args([
            "plugin",
            "marketplace",
            "add",
            "--sparse",
            ".agents",
            source.path().to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(contains(
            "--sparse is only supported for git marketplace sources",
        ));

    Ok(())
}
