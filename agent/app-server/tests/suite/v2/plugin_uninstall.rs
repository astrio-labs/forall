use std::time::Duration;

use anyhow::Result;
use anyhow::bail;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::PluginUninstallParams;
use forall_app_server_protocol::PluginUninstallResponse;
use forall_app_server_protocol::RequestId;
use forall_config::types::AuthCredentialsStoreMode;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const REMOTE_PLUGIN_ID: &str = "plugins~Plugin_linear";
const WORKSPACE_REMOTE_PLUGIN_ID: &str = "plugins_69f27c3e67848191a45cbaa5f2adb39d";

#[tokio::test]
async fn plugin_uninstall_removes_plugin_cache_and_config_entry() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_installed_plugin(&forall_home, "debug", "sample-plugin")?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."sample-plugin@debug"]
enabled = true
"#,
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let params = PluginUninstallParams {
        plugin_id: "sample-plugin@debug".to_string(),
    };

    let request_id = mcp.send_plugin_uninstall_request(params.clone()).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;
    assert_eq!(response, PluginUninstallResponse {});

    assert!(
        !forall_home
            .path()
            .join("plugins/cache/debug/sample-plugin")
            .exists()
    );
    let config = std::fs::read_to_string(forall_home.path().join("config.toml"))?;
    assert!(!config.contains(r#"[plugins."sample-plugin@debug"]"#));

    let request_id = mcp.send_plugin_uninstall_request(params).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;
    assert_eq!(response, PluginUninstallResponse {});

    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_remote_plugin_when_plugins_are_disabled() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"[features]
plugins = false
"#,
    )?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "plugins~Plugin_sample".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(
        err.error
            .message
            .contains("remote plugin uninstall is not enabled")
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_uninstall_writes_remote_plugin_to_cloud_when_remote_plugin_enabled() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    mount_remote_plugin_detail(&server, REMOTE_PLUGIN_ID, "1.0.0", "GLOBAL").await;

    Mock::given(method("POST"))
        .and(path(format!(
            "/backend-api/ps/plugins/{REMOTE_PLUGIN_ID}/uninstall"
        )))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"id":"{REMOTE_PLUGIN_ID}","enabled":false}}"#)),
        )
        .mount(&server)
        .await;

    let remote_plugin_cache_root = forall_home
        .path()
        .join("plugins/cache/openai-curated-remote/linear");
    std::fs::create_dir_all(remote_plugin_cache_root.join("1.0.0/.forall-plugin"))?;
    std::fs::write(
        remote_plugin_cache_root.join("1.0.0/.forall-plugin/plugin.json"),
        r#"{"name":"linear","version":"1.0.0"}"#,
    )?;
    let legacy_remote_plugin_cache_root = forall_home.path().join(format!(
        "plugins/cache/openai-curated-remote/{REMOTE_PLUGIN_ID}"
    ));
    std::fs::create_dir_all(legacy_remote_plugin_cache_root.join("local/.forall-plugin"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: REMOTE_PLUGIN_ID.to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;

    assert_eq!(response, PluginUninstallResponse {});
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/uninstall"),
        /*expected_count*/ 1,
    )
    .await?;
    assert!(!remote_plugin_cache_root.exists());
    assert!(!legacy_remote_plugin_cache_root.exists());
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_uninstall_uses_detail_scope_for_cache_namespace() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;
    mount_remote_plugin_detail(&server, REMOTE_PLUGIN_ID, "1.0.0", "WORKSPACE").await;

    Mock::given(method("POST"))
        .and(path(format!(
            "/backend-api/ps/plugins/{REMOTE_PLUGIN_ID}/uninstall"
        )))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"id":"{REMOTE_PLUGIN_ID}","enabled":false}}"#)),
        )
        .mount(&server)
        .await;

    let workspace_cache_root = forall_home
        .path()
        .join("plugins/cache/workspace-directory/linear");
    std::fs::create_dir_all(workspace_cache_root.join("1.0.0/.forall-plugin"))?;
    std::fs::write(
        workspace_cache_root.join("1.0.0/.forall-plugin/plugin.json"),
        r#"{"name":"linear","version":"1.0.0"}"#,
    )?;
    let global_cache_root = forall_home
        .path()
        .join("plugins/cache/openai-curated-remote/linear");
    std::fs::create_dir_all(global_cache_root.join("1.0.0/.forall-plugin"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: REMOTE_PLUGIN_ID.to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;

    assert_eq!(response, PluginUninstallResponse {});
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/uninstall"),
        /*expected_count*/ 1,
    )
    .await?;
    assert!(!workspace_cache_root.exists());
    assert!(global_cache_root.exists());
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_uninstall_accepts_workspace_remote_plugin_id_shape() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;
    mount_remote_plugin_detail_with_name(
        &server,
        WORKSPACE_REMOTE_PLUGIN_ID,
        "skill-improver",
        "1.0.0",
        "WORKSPACE",
    )
    .await;

    Mock::given(method("POST"))
        .and(path(format!(
            "/backend-api/ps/plugins/{WORKSPACE_REMOTE_PLUGIN_ID}/uninstall"
        )))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"{{"id":"{WORKSPACE_REMOTE_PLUGIN_ID}","enabled":false}}"#
        )))
        .mount(&server)
        .await;

    let remote_plugin_cache_root = forall_home
        .path()
        .join("plugins/cache/workspace-directory/skill-improver");
    std::fs::create_dir_all(remote_plugin_cache_root.join("1.0.0/.forall-plugin"))?;
    std::fs::write(
        remote_plugin_cache_root.join("1.0.0/.forall-plugin/plugin.json"),
        r#"{"name":"skill-improver","version":"1.0.0"}"#,
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: WORKSPACE_REMOTE_PLUGIN_ID.to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;

    assert_eq!(response, PluginUninstallResponse {});
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{WORKSPACE_REMOTE_PLUGIN_ID}/uninstall"),
        /*expected_count*/ 1,
    )
    .await?;
    assert!(!remote_plugin_cache_root.exists());
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_uninstall_rejects_before_post_when_remote_detail_fetch_fails() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let legacy_remote_plugin_cache_root = forall_home.path().join(format!(
        "plugins/cache/openai-curated-remote/{REMOTE_PLUGIN_ID}"
    ));
    std::fs::create_dir_all(legacy_remote_plugin_cache_root.join("local/.forall-plugin"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: REMOTE_PLUGIN_ID.to_string(),
        })
        .await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("remote plugin catalog request"));
    wait_for_remote_plugin_request_count(
        &server,
        "GET",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}"),
        /*expected_count*/ 1,
    )
    .await?;
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/uninstall"),
        /*expected_count*/ 0,
    )
    .await?;
    assert!(legacy_remote_plugin_cache_root.exists());
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_remote_plugin_id_with_spaces_before_network_call() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "sample plugin".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        "/ps/plugins/sample plugin/uninstall",
        /*expected_count*/ 0,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_invalid_remote_plugin_id_before_network_call() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "linear/../../oops".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        "/ps/plugins/linear/../../oops/uninstall",
        /*expected_count*/ 0,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_empty_remote_plugin_id() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: String::new(),
        })
        .await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));

    Ok(())
}

fn write_installed_plugin(
    forall_home: &TempDir,
    marketplace_name: &str,
    plugin_name: &str,
) -> Result<()> {
    let plugin_root = forall_home
        .path()
        .join("plugins/cache")
        .join(marketplace_name)
        .join(plugin_name)
        .join("local/.forall-plugin");
    std::fs::create_dir_all(&plugin_root)?;
    std::fs::write(
        plugin_root.join("plugin.json"),
        format!(r#"{{"name":"{plugin_name}"}}"#),
    )?;
    Ok(())
}

fn write_remote_plugin_catalog_config(
    forall_home: &std::path::Path,
    _base_url: &str,
) -> std::io::Result<()> {
    std::fs::write(
        forall_home.join("config.toml"),
        r#"

[features]
plugins = true
remote_plugin = true
"#,
    )
}

async fn mount_remote_plugin_detail(
    server: &MockServer,
    remote_plugin_id: &str,
    release_version: &str,
    scope: &str,
) {
    mount_remote_plugin_detail_with_name(
        server,
        remote_plugin_id,
        "linear",
        release_version,
        scope,
    )
    .await;
}

async fn mount_remote_plugin_detail_with_name(
    server: &MockServer,
    remote_plugin_id: &str,
    plugin_name: &str,
    release_version: &str,
    scope: &str,
) {
    let discoverability = if scope == "WORKSPACE" {
        r#"
  "discoverability": "LISTED","#
    } else {
        ""
    };
    let detail_body = format!(
        r#"{{
  "id": "{remote_plugin_id}",
  "name": "{plugin_name}",
  "scope": "{scope}",{discoverability}
  "installation_policy": "AVAILABLE",
  "authentication_policy": "ON_USE",
  "release": {{
    "version": "{release_version}",
    "display_name": "Linear",
    "description": "Track work in Linear",
    "interface": {{
      "short_description": "Plan and track work"
    }},
    "skills": []
  }}
}}"#
    );

    Mock::given(method("GET"))
        .and(path(format!("/backend-api/ps/plugins/{remote_plugin_id}")))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail_body))
        .mount(server)
        .await;
}

async fn wait_for_remote_plugin_request_count(
    server: &MockServer,
    method_name: &str,
    path_suffix: &str,
    expected_count: usize,
) -> Result<()> {
    timeout(DEFAULT_TIMEOUT, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                if expected_count == 0 {
                    return Ok::<(), anyhow::Error>(());
                }
                bail!("wiremock did not record requests");
            };
            let request_count = requests
                .iter()
                .filter(|request| {
                    request.method == method_name && request.url.path().ends_with(path_suffix)
                })
                .count();
            if request_count == expected_count {
                return Ok::<(), anyhow::Error>(());
            }
            if request_count > expected_count {
                bail!(
                    "expected exactly {expected_count} {method_name} {path_suffix} requests, got {request_count}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}
