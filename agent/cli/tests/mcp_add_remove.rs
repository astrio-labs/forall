use std::path::Path;

use anyhow::Result;
use forall_config::types::McpServerTransportConfig;
use forall_core::config::load_global_mcp_servers;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn forall_command(forall_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(forall_utils_cargo_bin::cargo_bin("forall")?);
    cmd.env("FORALL_HOME", forall_home);
    Ok(cmd)
}

#[tokio::test]
async fn add_and_remove_server_updates_global_config() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args(["mcp", "add", "docs", "--", "echo", "hello"])
        .assert()
        .success()
        .stdout(contains("Added global MCP server 'docs'."));

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    assert_eq!(servers.len(), 1);
    let docs = servers.get("docs").expect("server should exist");
    match &docs.transport {
        McpServerTransportConfig::Stdio {
            command,
            args,
            env,
            env_vars,
            cwd,
        } => {
            assert_eq!(command, "echo");
            assert_eq!(args, &vec!["hello".to_string()]);
            assert!(env.is_none());
            assert!(env_vars.is_empty());
            assert!(cwd.is_none());
        }
        other => panic!("unexpected transport: {other:?}"),
    }
    assert!(docs.enabled);

    let mut remove_cmd = forall_command(forall_home.path())?;
    remove_cmd
        .args(["mcp", "remove", "docs"])
        .assert()
        .success()
        .stdout(contains("Removed global MCP server 'docs'."));

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    assert!(servers.is_empty());

    let mut remove_again_cmd = forall_command(forall_home.path())?;
    remove_again_cmd
        .args(["mcp", "remove", "docs"])
        .assert()
        .success()
        .stdout(contains("No MCP server named 'docs' found."));

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    assert!(servers.is_empty());

    Ok(())
}

#[tokio::test]
async fn profile_mcp_reports_legacy_profile_migration() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"[profiles.work]
model = "gpt-5"
"#,
    )?;

    let mut list_cmd = forall_command(forall_home.path())?;
    list_cmd
        .args(["--profile", "work", "mcp", "list"])
        .assert()
        .failure()
        .stderr(contains("--profile `work` cannot be used"))
        .stderr(contains("[profiles.work]"))
        .stderr(contains("work.config.toml"));

    Ok(())
}

#[tokio::test]
async fn add_with_env_preserves_key_order_and_values() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args([
            "mcp",
            "add",
            "envy",
            "--env",
            "FOO=bar",
            "--env",
            "ALPHA=beta",
            "--",
            "python",
            "server.py",
        ])
        .assert()
        .success();

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    let envy = servers.get("envy").expect("server should exist");
    let env = match &envy.transport {
        McpServerTransportConfig::Stdio { env: Some(env), .. } => env,
        other => panic!("unexpected transport: {other:?}"),
    };

    assert_eq!(env.len(), 2);
    assert_eq!(env.get("FOO"), Some(&"bar".to_string()));
    assert_eq!(env.get("ALPHA"), Some(&"beta".to_string()));
    assert!(envy.enabled);

    Ok(())
}

#[tokio::test]
async fn add_streamable_http_without_manual_token() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args(["mcp", "add", "github", "--url", "https://example.com/mcp"])
        .assert()
        .success();

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    let github = servers.get("github").expect("github server should exist");
    match &github.transport {
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => {
            assert_eq!(url, "https://example.com/mcp");
            assert!(bearer_token_env_var.is_none());
            assert!(http_headers.is_none());
            assert!(env_http_headers.is_none());
        }
        other => panic!("unexpected transport: {other:?}"),
    }
    assert!(github.enabled);

    assert!(!forall_home.path().join(".credentials.json").exists());
    assert!(!forall_home.path().join(".env").exists());

    Ok(())
}

#[tokio::test]
async fn add_streamable_http_with_custom_env_var() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args([
            "mcp",
            "add",
            "issues",
            "--url",
            "https://example.com/issues",
            "--bearer-token-env-var",
            "GITHUB_TOKEN",
        ])
        .assert()
        .success();

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    let issues = servers.get("issues").expect("issues server should exist");
    match &issues.transport {
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => {
            assert_eq!(url, "https://example.com/issues");
            assert_eq!(bearer_token_env_var.as_deref(), Some("GITHUB_TOKEN"));
            assert!(http_headers.is_none());
            assert!(env_http_headers.is_none());
        }
        other => panic!("unexpected transport: {other:?}"),
    }
    assert!(issues.enabled);
    Ok(())
}

#[tokio::test]
async fn add_streamable_http_with_oauth_options() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args([
            "mcp",
            "add",
            "oauth-server",
            "--url",
            "https://example.com/mcp",
            "--oauth-client-id",
            "eci-prd-pub-forall-123",
            "--oauth-resource",
            "https://resource.example.com",
        ])
        .assert()
        .success();

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    let oauth_server = servers
        .get("oauth-server")
        .expect("oauth server should exist");
    assert_eq!(
        oauth_server.oauth_client_id(),
        Some("eci-prd-pub-forall-123")
    );
    assert_eq!(
        oauth_server.oauth_resource.as_deref(),
        Some("https://resource.example.com")
    );

    Ok(())
}

#[tokio::test]
async fn add_streamable_http_rejects_removed_flag() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args([
            "mcp",
            "add",
            "github",
            "--url",
            "https://example.com/mcp",
            "--with-bearer-token",
        ])
        .assert()
        .failure()
        .stderr(contains("--with-bearer-token"));

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    assert!(servers.is_empty());

    Ok(())
}

#[tokio::test]
async fn add_cant_add_command_and_url() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut add_cmd = forall_command(forall_home.path())?;
    add_cmd
        .args([
            "mcp",
            "add",
            "github",
            "--url",
            "https://example.com/mcp",
            "--command",
            "--",
            "echo",
            "hello",
        ])
        .assert()
        .failure()
        .stderr(contains("unexpected argument '--command' found"));

    let servers = load_global_mcp_servers(forall_home.path()).await?;
    assert!(servers.is_empty());

    Ok(())
}
