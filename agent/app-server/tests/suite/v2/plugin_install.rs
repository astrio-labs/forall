use std::time::Duration;

use anyhow::Result;
use anyhow::bail;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use flate2::Compression;
use flate2::write::GzEncoder;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::PluginAuthPolicy;
use forall_app_server_protocol::PluginAvailability;
use forall_app_server_protocol::PluginInstallParams;
use forall_app_server_protocol::PluginInstallResponse;
use forall_app_server_protocol::RequestId;
use forall_config::types::AuthCredentialsStoreMode;
use forall_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Match;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

// Plugin install tests wait on connector discovery after the install response path
// starts, which is noticeably slower on Windows CI.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_PLUGIN_ID: &str = "plugins~Plugin_00000000000000000000000000000000";
const TEST_ALLOW_HTTP_REMOTE_PLUGIN_BUNDLE_DOWNLOADS: &str =
    "CODEX_TEST_ALLOW_HTTP_REMOTE_PLUGIN_BUNDLE_DOWNLOADS";

#[tokio::test]
async fn plugin_install_rejects_relative_marketplace_paths() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/install",
            Some(serde_json::json!({
                "marketplacePath": "relative-marketplace.json",
                "pluginName": "missing-plugin",
            })),
        )
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("Invalid request"));
    Ok(())
}

#[tokio::test]
async fn plugin_install_rejects_missing_install_source() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: None,
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
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
            .contains("requires exactly one of marketplacePath or remoteMarketplaceName")
    );
    Ok(())
}

#[tokio::test]
async fn plugin_install_rejects_multiple_install_sources() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(AbsolutePathBuf::try_from(
                forall_home.path().join("marketplace.json"),
            )?),
            remote_marketplace_name: Some("openai-curated-remote".to_string()),
            plugin_name: "sample-plugin".to_string(),
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
            .contains("requires exactly one of marketplacePath or remoteMarketplaceName")
    );
    Ok(())
}

#[tokio::test]
async fn plugin_install_rejects_remote_marketplace_when_plugins_are_disabled() -> Result<()> {
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
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: None,
            remote_marketplace_name: Some("openai-curated-remote".to_string()),
            plugin_name: "plugins~Plugin_22222222222222222222222222222222".to_string(),
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
            .contains("remote plugin install is not enabled")
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_writes_remote_plugin_to_cloud_and_cache() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    let installed_path = forall_home
        .path()
        .join("plugins/cache/openai-curated-remote/linear/1.2.3");
    let bundle_url = mount_remote_plugin_bundle(
        &server,
        /*status_code*/ 200,
        remote_plugin_bundle_tar_gz_bytes_with_contents(r#"{"name":"linear","version":"0.0.1"}"#)?,
    )
    .await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail(&server, REMOTE_PLUGIN_ID, "1.2.3", Some(&bundle_url)).await;
    mount_empty_remote_installed_plugins(&server).await;
    mount_remote_plugin_install_after_cache_write(
        &server,
        REMOTE_PLUGIN_ID,
        installed_path.join(".forall-plugin/plugin.json"),
    )
    .await;

    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[(TEST_ALLOW_HTTP_REMOTE_PLUGIN_BUNDLE_DOWNLOADS, Some("1"))],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginInstallResponse = to_response(response)?;

    assert_eq!(
        response,
        PluginInstallResponse {
            auth_policy: PluginAuthPolicy::OnUse,
        }
    );
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 1,
    )
    .await?;
    wait_for_remote_plugin_request_count(
        &server,
        "GET",
        "/bundles/linear.tar.gz",
        /*expected_count*/ 1,
    )
    .await?;
    assert!(installed_path.join(".forall-plugin/plugin.json").is_file());
    let installed_plugin_manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(installed_path.join(".forall-plugin/plugin.json"))?,
    )?;
    assert_eq!(installed_plugin_manifest["name"], json!("linear"));
    assert_eq!(installed_plugin_manifest["version"], json!("1.2.3"));
    assert!(installed_path.join("skills/plan-work/SKILL.md").is_file());
    assert!(
        !forall_home
            .path()
            .join(format!(
                "plugins/cache/openai-curated-remote/{REMOTE_PLUGIN_ID}/1.2.3"
            ))
            .exists()
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_missing_remote_bundle_url() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail(
        &server,
        REMOTE_PLUGIN_ID,
        "1.2.3",
        /*bundle_download_url*/ None,
    )
    .await;
    mount_empty_remote_installed_plugins(&server).await;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32603);
    assert!(
        err.error
            .message
            .contains("backend did not return a download URL")
    );
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    assert!(
        !forall_home
            .path()
            .join("plugins/cache/openai-curated-remote/linear")
            .exists()
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_plain_http_remote_bundle_url() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    let bundle_url = format!("{}/bundles/linear.tar.gz", server.uri());
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail(&server, REMOTE_PLUGIN_ID, "1.2.3", Some(&bundle_url)).await;
    mount_empty_remote_installed_plugins(&server).await;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32603);
    assert!(
        err.error
            .message
            .contains("unsupported download URL scheme")
    );
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    assert!(
        !forall_home
            .path()
            .join("plugins/cache/openai-curated-remote/linear")
            .exists()
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_invalid_remote_release_version() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail(
        &server,
        REMOTE_PLUGIN_ID,
        "../1.2.3",
        Some("https://127.0.0.1:1/bundles/linear.tar.gz"),
    )
    .await;
    mount_empty_remote_installed_plugins(&server).await;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32603);
    assert!(err.error.message.contains("invalid release version"));
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    assert!(
        !forall_home
            .path()
            .join("plugins/cache/openai-curated-remote/linear")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn plugin_install_rejects_invalid_remote_plugin_name() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_remote_plugin_catalog_config(forall_home.path(), "https://example.invalid/backend-api/")?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: None,
            remote_marketplace_name: Some("openai-curated-remote".to_string()),
            plugin_name: "linear/../../oops".to_string(),
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

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_remote_plugin_disabled_by_admin_before_download() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    let bundle_url = mount_remote_plugin_bundle(
        &server,
        /*status_code*/ 200,
        remote_plugin_bundle_tar_gz_bytes("linear")?,
    )
    .await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail_with_status(
        &server,
        REMOTE_PLUGIN_ID,
        "1.2.3",
        Some(&bundle_url),
        PluginAvailability::DisabledByAdmin,
    )
    .await;
    mount_empty_remote_installed_plugins(&server).await;

    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[(TEST_ALLOW_HTTP_REMOTE_PLUGIN_BUNDLE_DOWNLOADS, Some("1"))],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("disabled by admin"));
    wait_for_remote_plugin_request_count(
        &server,
        "GET",
        "/bundles/linear.tar.gz",
        /*expected_count*/ 0,
    )
    .await?;
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    assert!(
        !forall_home
            .path()
            .join("plugins/cache/openai-curated-remote/linear")
            .exists()
    );
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_remote_plugin_not_available() -> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail_with_install_policy(
        &server,
        REMOTE_PLUGIN_ID,
        "1.2.3",
        /*install_policy*/ "NOT_AVAILABLE",
    )
    .await;
    mount_empty_remote_installed_plugins(&server).await;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("not available for install"));
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_rejects_when_workspace_forall_plugins_disabled() -> Result<()> {
    let forall_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    let server = MockServer::start().await;
    write_plugins_enabled_config_with_base_url(
        forall_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123")
            .plan_type("team"),
        AuthCredentialsStoreMode::File,
    )?;
    write_plugin_marketplace(
        repo_root.path(),
        "debug",
        "sample-plugin",
        "./sample-plugin",
        /*install_policy*/ None,
        /*auth_policy*/ None,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    Mock::given(method("GET"))
        .and(path("/backend-api/accounts/account-123/settings"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"beta_settings":{"enable_plugins":false}}"#),
        )
        .mount(&server)
        .await;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
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
            .contains("Forall plugins are disabled for this workspace")
    );
    Ok(())
}

#[tokio::test]
async fn plugin_install_returns_invalid_request_for_missing_marketplace_file() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(AbsolutePathBuf::try_from(
                forall_home.path().join("missing-marketplace.json"),
            )?),
            remote_marketplace_name: None,
            plugin_name: "missing-plugin".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("marketplace file"));
    assert!(err.error.message.contains("does not exist"));
    Ok(())
}

#[tokio::test]
async fn plugin_install_returns_invalid_request_for_not_available_plugin() -> Result<()> {
    let forall_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    write_plugin_marketplace(
        repo_root.path(),
        "debug",
        "sample-plugin",
        "./sample-plugin",
        Some("NOT_AVAILABLE"),
        /*auth_policy*/ None,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("not available for install"));
    Ok(())
}

#[tokio::test]
async fn plugin_install_returns_invalid_request_for_disallowed_product_plugin() -> Result<()> {
    let forall_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "debug",
  "plugins": [
    {
      "name": "sample-plugin",
      "source": {
        "source": "local",
        "path": "./sample-plugin"
      },
      "policy": {
        "products": ["CHATGPT"]
      }
    }
  ]
}"#,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    let mut mcp =
        TestAppServer::new_with_args(forall_home.path(), &["--session-source", "atlas"]).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("not available for install"));
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn plugin_install_preserves_status_when_remote_bundle_error_body_is_too_large() -> Result<()>
{
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    let bundle_url =
        mount_remote_plugin_bundle(&server, /*status_code*/ 503, vec![b'x'; 8 * 1024 + 1]).await;
    configure_remote_plugin_test(forall_home.path(), &server)?;
    mount_remote_plugin_detail(&server, REMOTE_PLUGIN_ID, "1.2.3", Some(&bundle_url)).await;
    mount_empty_remote_installed_plugins(&server).await;
    mount_remote_plugin_install(&server, REMOTE_PLUGIN_ID).await;

    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[(TEST_ALLOW_HTTP_REMOTE_PLUGIN_BUNDLE_DOWNLOADS, Some("1"))],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = send_remote_plugin_install_request(&mut mcp, REMOTE_PLUGIN_ID).await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32603);
    assert!(err.error.message.contains("failed with status 503"));
    assert!(
        err.error
            .message
            .contains("[response body truncated after 8192 bytes]")
    );
    assert_eq!(
        err.error
            .message
            .bytes()
            .filter(|byte| *byte == b'x')
            .count(),
        8192
    );
    assert!(!err.error.message.contains("exceeded maximum size"));
    wait_for_remote_plugin_request_count(
        &server,
        "GET",
        "/bundles/linear.tar.gz",
        /*expected_count*/ 1,
    )
    .await?;
    wait_for_remote_plugin_request_count(
        &server,
        "POST",
        &format!("/ps/plugins/{REMOTE_PLUGIN_ID}/install"),
        /*expected_count*/ 0,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn plugin_install_starts_mcp_oauth_through_protected_resource_metadata() -> Result<()> {
    let resource_server = MockServer::start().await;
    let authorization_server = MockServer::start().await;
    let resource_metadata_url = format!("{}/oauth-resource", resource_server.uri());
    let challenge = format!("Bearer resource_metadata=\"{resource_metadata_url}\"");
    Mock::given(method("GET"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(401).insert_header("WWW-Authenticate", challenge.as_str()),
        )
        .mount(&resource_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/oauth-resource"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "resource": resource_server.uri(),
            "authorization_servers": [authorization_server.uri()],
        })))
        .mount(&resource_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/.well-known/oauth-authorization-server"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_endpoint": format!("{}/oauth/authorize", authorization_server.uri()),
            "token_endpoint": format!("{}/oauth/token", authorization_server.uri()),
            "registration_endpoint": format!("{}/oauth/register", authorization_server.uri()),
            "response_types_supported": ["code"],
            "code_challenge_methods_supported": ["S256"],
        })))
        .mount(&authorization_server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/register"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&authorization_server)
        .await;

    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        "[features]\nplugins = true\n",
    )?;
    let repo_root = TempDir::new()?;
    write_plugin_marketplace(
        repo_root.path(),
        "debug",
        "sample-plugin",
        "./sample-plugin",
        /*install_policy*/ None,
        /*auth_policy*/ None,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    write_plugin_mcp_config(repo_root.path(), "sample-plugin", &resource_server.uri())?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: PluginInstallResponse = to_response(response)?;
    wait_for_remote_plugin_request_count(
        &authorization_server,
        "POST",
        "/oauth/register",
        /*expected_count*/ 1,
    )
    .await?;

    let resource_metadata_requested = resource_server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .any(|request| request.url.path() == "/oauth-resource");
    assert!(resource_metadata_requested);
    Ok(())
}

#[tokio::test]
async fn plugin_install_starts_mcp_oauth_for_api_key_dual_surface_plugin() -> Result<()> {
    let oauth_server = MockServer::start().await;
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"
mcp_oauth_credentials_store = "file"

[features]
plugins = true
connectors = true
"#,
    )?;

    let repo_root = TempDir::new()?;
    write_plugin_marketplace(
        repo_root.path(),
        "debug",
        "sample-plugin",
        "./sample-plugin",
        /*install_policy*/ None,
        /*auth_policy*/ None,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    write_plugin_mcp_config(repo_root.path(), "sample-plugin", &oauth_server.uri())?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[("OPENAI_API_KEY", Some("test-api-key"))],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginInstallResponse = to_response(response)?;

    assert_eq!(response.auth_policy, PluginAuthPolicy::OnInstall);
    assert!(oauth_discovery_request_count(&oauth_server).await > 0);
    Ok(())
}

#[tokio::test]
async fn plugin_install_makes_bundled_mcp_servers_available_to_followup_requests() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        "[features]\nplugins = true\n",
    )?;
    let repo_root = TempDir::new()?;
    write_plugin_marketplace(
        repo_root.path(),
        "debug",
        "sample-plugin",
        "./sample-plugin",
        /*install_policy*/ None,
        /*auth_policy*/ None,
    )?;
    write_plugin_source(repo_root.path(), "sample-plugin")?;
    std::fs::write(
        repo_root.path().join("sample-plugin/.mcp.json"),
        r#"{
  "mcpServers": {
    "sample-mcp": {
      "command": "echo"
    }
  }
}"#,
    )?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_install_request(PluginInstallParams {
            marketplace_path: Some(marketplace_path),
            remote_marketplace_name: None,
            plugin_name: "sample-plugin".to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: PluginInstallResponse = to_response(response)?;
    let config = std::fs::read_to_string(forall_home.path().join("config.toml"))?;
    assert!(!config.contains("[mcp_servers.sample-mcp]"));
    assert!(!config.contains("command = \"echo\""));

    let request_id = mcp
        .send_raw_request(
            "mcpServer/oauth/login",
            Some(json!({
                "name": "sample-mcp",
            })),
        )
        .await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert_eq!(
        err.error.message,
        "OAuth login is only supported for streamable HTTP servers."
    );
    Ok(())
}

fn write_plugins_enabled_config_with_base_url(
    forall_home: &std::path::Path,
    _base_url: &str,
) -> std::io::Result<()> {
    std::fs::write(
        forall_home.join("config.toml"),
        r#"[features]
plugins = true
"#,
    )
}

async fn oauth_discovery_request_count(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.url.path().contains("oauth-authorization-server"))
        .count()
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

fn configure_remote_plugin_test(forall_home: &std::path::Path, server: &MockServer) -> Result<()> {
    write_remote_plugin_catalog_config(forall_home, &format!("{}/backend-api/", server.uri()))?;
    write_api_key_auth(
        forall_home,
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )
}

async fn mount_remote_plugin_bundle(
    server: &MockServer,
    status_code: u16,
    body: Vec<u8>,
) -> String {
    Mock::given(method("GET"))
        .and(path("/bundles/linear.tar.gz"))
        .respond_with(
            ResponseTemplate::new(status_code)
                .insert_header("content-type", "application/gzip")
                .set_body_bytes(body),
        )
        .mount(server)
        .await;
    format!("{}/bundles/linear.tar.gz", server.uri())
}

async fn mount_remote_plugin_detail(
    server: &MockServer,
    remote_plugin_id: &str,
    release_version: &str,
    bundle_download_url: Option<&str>,
) {
    mount_remote_plugin_detail_with_status(
        server,
        remote_plugin_id,
        release_version,
        bundle_download_url,
        PluginAvailability::Available,
    )
    .await;
}

async fn mount_remote_plugin_detail_with_status(
    server: &MockServer,
    remote_plugin_id: &str,
    release_version: &str,
    bundle_download_url: Option<&str>,
    status: PluginAvailability,
) {
    mount_remote_plugin_detail_with_options(
        server,
        remote_plugin_id,
        release_version,
        bundle_download_url,
        status,
        "AVAILABLE",
    )
    .await;
}

async fn mount_remote_plugin_detail_with_install_policy(
    server: &MockServer,
    remote_plugin_id: &str,
    release_version: &str,
    install_policy: &str,
) {
    mount_remote_plugin_detail_with_options(
        server,
        remote_plugin_id,
        release_version,
        /*bundle_download_url*/ None,
        PluginAvailability::Available,
        install_policy,
    )
    .await;
}

async fn mount_remote_plugin_detail_with_options(
    server: &MockServer,
    remote_plugin_id: &str,
    release_version: &str,
    bundle_download_url: Option<&str>,
    status: PluginAvailability,
    install_policy: &str,
) {
    let status = match status {
        PluginAvailability::Available => "ENABLED",
        PluginAvailability::DisabledByAdmin => "DISABLED_BY_ADMIN",
    };
    let bundle_download_url_field = bundle_download_url
        .map(|url| format!(r#"    "bundle_download_url": "{url}","#))
        .unwrap_or_default();
    let detail_body = format!(
        r#"{{
  "id": "{remote_plugin_id}",
  "name": "linear",
  "scope": "GLOBAL",
  "installation_policy": "{install_policy}",
  "authentication_policy": "ON_USE",
  "status": "{status}",
  "release": {{
    "version": "{release_version}",
{bundle_download_url_field}
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
        .and(query_param("includeDownloadUrls", "true"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string(detail_body))
        .mount(server)
        .await;
}

async fn mount_empty_remote_installed_plugins(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/backend-api/ps/plugins/installed"))
        .and(query_param("scope", "GLOBAL"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{
  "plugins": [],
  "pagination": {
    "limit": 50,
    "next_page_token": null
  }
}"#,
        ))
        .mount(server)
        .await;
}

async fn mount_remote_plugin_install(server: &MockServer, remote_plugin_id: &str) {
    Mock::given(method("POST"))
        .and(path(format!(
            "/backend-api/ps/plugins/{remote_plugin_id}/install"
        )))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"id":"{remote_plugin_id}","enabled":true}}"#)),
        )
        .mount(server)
        .await;
}

#[derive(Debug, Clone)]
struct CacheManifestExists {
    manifest_path: std::path::PathBuf,
}

impl Match for CacheManifestExists {
    fn matches(&self, _request: &Request) -> bool {
        self.manifest_path.is_file()
    }
}

async fn mount_remote_plugin_install_after_cache_write(
    server: &MockServer,
    remote_plugin_id: &str,
    manifest_path: std::path::PathBuf,
) {
    Mock::given(method("POST"))
        .and(path(format!(
            "/backend-api/ps/plugins/{remote_plugin_id}/install"
        )))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .and(CacheManifestExists { manifest_path })
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"id":"{remote_plugin_id}","enabled":true}}"#)),
        )
        .mount(server)
        .await;
}

async fn send_remote_plugin_install_request(
    mcp: &mut TestAppServer,
    remote_plugin_id: &str,
) -> Result<i64> {
    mcp.send_plugin_install_request(PluginInstallParams {
        marketplace_path: None,
        remote_marketplace_name: Some("caller-marketplace-is-ignored".to_string()),
        plugin_name: remote_plugin_id.to_string(),
    })
    .await
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

fn write_plugin_marketplace(
    repo_root: &std::path::Path,
    marketplace_name: &str,
    plugin_name: &str,
    source_path: &str,
    install_policy: Option<&str>,
    auth_policy: Option<&str>,
) -> std::io::Result<()> {
    let policy = if install_policy.is_some() || auth_policy.is_some() {
        let installation = install_policy
            .map(|installation| format!("\n        \"installation\": \"{installation}\""))
            .unwrap_or_default();
        let separator = if install_policy.is_some() && auth_policy.is_some() {
            ","
        } else {
            ""
        };
        let authentication = auth_policy
            .map(|authentication| {
                format!("{separator}\n        \"authentication\": \"{authentication}\"")
            })
            .unwrap_or_default();
        format!(",\n      \"policy\": {{{installation}{authentication}\n      }}")
    } else {
        String::new()
    };
    std::fs::create_dir_all(repo_root.join(".git"))?;
    std::fs::create_dir_all(repo_root.join(".agents/plugins"))?;
    std::fs::write(
        repo_root.join(".agents/plugins/marketplace.json"),
        format!(
            r#"{{
  "name": "{marketplace_name}",
  "plugins": [
    {{
      "name": "{plugin_name}",
      "source": {{
        "source": "local",
        "path": "{source_path}"
      }}{policy}
    }}
  ]
}}"#
        ),
    )
}

fn write_plugin_source(repo_root: &std::path::Path, plugin_name: &str) -> Result<()> {
    let plugin_root = repo_root.join(plugin_name);
    std::fs::create_dir_all(plugin_root.join(".forall-plugin"))?;
    std::fs::write(
        plugin_root.join(".forall-plugin/plugin.json"),
        format!(r#"{{"name":"{plugin_name}"}}"#),
    )?;
    Ok(())
}

fn write_plugin_mcp_config(
    repo_root: &std::path::Path,
    plugin_name: &str,
    mcp_base_url: &str,
) -> Result<()> {
    std::fs::write(
        repo_root.join(plugin_name).join(".mcp.json"),
        format!(
            r#"{{
  "mcpServers": {{
    "sample-mcp": {{
      "type": "http",
      "url": "{mcp_base_url}/mcp"
    }}
  }}
}}"#
        ),
    )?;
    Ok(())
}

fn remote_plugin_bundle_tar_gz_bytes(plugin_name: &str) -> Result<Vec<u8>> {
    let manifest = format!(r#"{{"name":"{plugin_name}"}}"#);
    remote_plugin_bundle_tar_gz_bytes_with_contents(&manifest)
}

fn remote_plugin_bundle_tar_gz_bytes_with_contents(plugin_manifest: &str) -> Result<Vec<u8>> {
    remote_plugin_bundle_tar_gz_bytes_with_entries(plugin_manifest, /*mcp_config*/ None)
}

fn remote_plugin_bundle_tar_gz_bytes_with_entries(
    plugin_manifest: &str,
    mcp_config: Option<&str>,
) -> Result<Vec<u8>> {
    let skill = "# Plan Work\n\nTrack work in Linear.\n";
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(encoder);
    let mut entries = vec![
        (
            ".forall-plugin/plugin.json",
            plugin_manifest.as_bytes(),
            /*mode*/ 0o644,
        ),
        (
            "skills/plan-work/SKILL.md",
            skill.as_bytes(),
            /*mode*/ 0o644,
        ),
    ];
    if let Some(mcp_config) = mcp_config {
        entries.push((".mcp.json", mcp_config.as_bytes(), /*mode*/ 0o644));
    }
    for (path, contents, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, path, contents)?;
    }
    Ok(tar.into_inner()?.finish()?)
}
