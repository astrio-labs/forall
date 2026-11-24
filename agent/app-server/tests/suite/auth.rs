use anyhow::Result;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use forall_app_server_protocol::AuthMode;
use forall_app_server_protocol::GetAuthStatusParams;
use forall_app_server_protocol::GetAuthStatusResponse;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::RequestId;
use forall_config::types::AuthCredentialsStoreMode;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn create_config_toml_custom_provider(
    forall_home: &Path,
    requires_openai_auth: bool,
) -> std::io::Result<()> {
    let config_toml = forall_home.join("config.toml");
    let requires_line = if requires_openai_auth {
        "requires_openai_auth = true\n"
    } else {
        ""
    };
    let contents = format!(
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"

model_provider = "mock_provider"

[features]
shell_snapshot = false

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "http://127.0.0.1:0/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
{requires_line}
"#
    );
    std::fs::write(config_toml, contents)
}

fn create_config_toml(forall_home: &Path) -> std::io::Result<()> {
    let config_toml = forall_home.join("config.toml");
    std::fs::write(
        config_toml,
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[features]
shell_snapshot = false
"#,
    )
}

async fn login_with_api_key_via_request(mcp: &mut TestAppServer, api_key: &str) -> Result<()> {
    let request_id = mcp.send_login_account_api_key_request(api_key).await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: LoginAccountResponse = to_response(resp)?;
    assert_eq!(response, LoginAccountResponse::ApiKey {});
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_no_auth() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path())?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, None, "expected no auth method");
    assert_eq!(status.auth_token, None, "expected no token");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path())?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, Some(AuthMode::ApiKey));
    assert_eq!(status.auth_token, Some("sk-test-key".to_string()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_when_auth_not_required() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml_custom_provider(forall_home.path(), /*requires_openai_auth*/ false)?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, None, "expected no auth method");
    assert_eq!(status.auth_token, None, "expected no token");
    assert_eq!(
        status.requires_openai_auth,
        Some(false),
        "requires_openai_auth should be false",
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_no_include_token() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path())?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let params = GetAuthStatusParams {
        include_token: None,
        refresh_token: Some(false),
    };
    let request_id = mcp.send_get_auth_status_request(params).await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, Some(AuthMode::ApiKey));
    assert!(status.auth_token.is_none(), "token must be omitted");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_refresh_requested() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path())?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(true),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(
        status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: Some("sk-test-key".to_string()),
            requires_openai_auth: Some(true),
        }
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_stored_api_key_fixture() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path())?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("sk-fixture-key"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, Some(AuthMode::ApiKey));
    assert_eq!(status.auth_token, Some("sk-fixture-key".to_string()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_api_key_succeeds_when_forced_chatgpt_legacy_config() -> Result<()> {
    let forall_home = TempDir::new()?;
    let config_toml = forall_home.path().join("config.toml");
    std::fs::write(
        config_toml,
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"
forced_login_method = "chatgpt"

[features]
shell_snapshot = false
"#,
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;
    Ok(())
}
