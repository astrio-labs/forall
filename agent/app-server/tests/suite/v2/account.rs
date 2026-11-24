use anyhow::Result;
use anyhow::bail;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use forall_app_server_protocol::Account;
use forall_app_server_protocol::AuthMode;
use forall_app_server_protocol::CancelLoginAccountParams;
use forall_app_server_protocol::CancelLoginAccountResponse;
use forall_app_server_protocol::CancelLoginAccountStatus;
use forall_app_server_protocol::GetAccountParams;
use forall_app_server_protocol::GetAccountResponse;
use forall_app_server_protocol::JSONRPCError;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::LogoutAccountResponse;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::ServerNotification;
use forall_config::types::AuthCredentialsStoreMode;
use forall_login::AuthKeyringBackendKind;
use forall_login::login_with_api_key;
use forall_login::login_with_bedrock_api_key;
use forall_protocol::account::AmazonBedrockCredentialSource;
use forall_protocol::account::PlanType;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use url::Url;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct CreateConfigTomlParams {
    forced_method: Option<String>,
    requires_openai_auth: Option<bool>,
    base_url: Option<String>,
    model_provider_id: Option<String>,
    extra_provider_config: Option<String>,
}

fn create_config_toml(forall_home: &Path, params: CreateConfigTomlParams) -> std::io::Result<()> {
    let config_toml = forall_home.join("config.toml");
    let base_url = params
        .base_url
        .unwrap_or_else(|| "http://127.0.0.1:0/v1".to_string());
    let forced_line = if let Some(method) = params.forced_method {
        format!("forced_login_method = \"{method}\"\n")
    } else {
        String::new()
    };
    let requires_line = match params.requires_openai_auth {
        Some(true) => "requires_openai_auth = true\n".to_string(),
        Some(false) => "requires_openai_auth = false\n".to_string(),
        None => String::new(),
    };
    let model_provider_id = params
        .model_provider_id
        .unwrap_or_else(|| "mock_provider".to_string());
    let provider_section = if model_provider_id == "amazon-bedrock" {
        // Built-in Bedrock only allows aws.profile / aws.region overrides.
        let extra = params.extra_provider_config.unwrap_or_default();
        format!(
            r#"model_provider = "amazon-bedrock"
{extra}
"#
        )
    } else {
        let extra = params.extra_provider_config.unwrap_or_default();
        format!(
            r#"model_provider = "{model_provider_id}"

[model_providers.{model_provider_id}]
name = "Mock provider for test"
base_url = "{base_url}"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
{requires_line}
{extra}
"#
        )
    };
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"
{forced_line}
{provider_section}
[features]
shell_snapshot = false
"#
        ),
    )
}

/// Builds an unsigned JWT-shaped token whose payload carries the Forall auth
/// claims namespace; signature verification is not performed by the client.
fn fake_jwt(payload: &serde_json::Value) -> Result<String> {
    let payload_bytes = serde_json::to_vec(payload)?;
    Ok(format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(b"{}"),
        URL_SAFE_NO_PAD.encode(payload_bytes),
        URL_SAFE_NO_PAD.encode(b"sig"),
    ))
}

fn fake_id_token() -> Result<String> {
    fake_jwt(&serde_json::json!({
        "email": "user@example.com",
        "https://forall.astrio.app/auth": {
            "chatgpt_user_id": "user-1",
            "chatgpt_account_id": "account-1",
            "chatgpt_plan_type": "pro",
        },
    }))
}

/// Starts an OAuth login and returns `(login_id, auth_url)`.
async fn start_oauth_login(mcp: &mut TestAppServer) -> Result<(String, String)> {
    let request_id = mcp.send_login_account_oauth_request().await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let login: LoginAccountResponse = to_response(response)?;
    let LoginAccountResponse::Oauth { login_id, auth_url } = login else {
        bail!("expected oauth login response, got: {login:?}");
    };
    Ok((login_id, auth_url))
}

#[tokio::test]
async fn login_account_oauth_completes_and_updates_account() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    // Mock Forall auth service: only the token endpoint is exercised because the
    // test drives the loopback callback directly instead of a real browser.
    let auth_server = MockServer::start().await;
    let id_token = fake_id_token()?;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": id_token,
            "access_token": id_token,
            "refresh_token": "refresh-1",
            "token_type": "bearer",
            "expires_in": 3600,
        })))
        .mount(&auth_server)
        .await;

    let auth_base_url = auth_server.uri();
    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("FORALL_AUTH_BASE_URL", Some(auth_base_url.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let (login_id, auth_url) = start_oauth_login(&mut mcp).await?;
    assert!(
        auth_url.starts_with(&format!("{auth_base_url}/oauth/authorize?")),
        "unexpected auth url: {auth_url}"
    );

    // The authorization URL carries the loopback redirect and CSRF state;
    // simulate the browser being redirected back to the loopback server.
    let query: HashMap<String, String> = Url::parse(&auth_url)?
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let redirect_uri = query
        .get("redirect_uri")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("auth url missing redirect_uri"))?;
    let state = query
        .get("state")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("auth url missing state"))?;
    assert!(redirect_uri.starts_with("http://127.0.0.1:"));
    assert_eq!(
        query.get("code_challenge_method").map(String::as_str),
        Some("S256")
    );

    let callback = format!("{redirect_uri}?code=fake-code&state={state}");
    let callback_response = reqwest::get(callback).await?;
    assert_eq!(callback_response.status(), reqwest::StatusCode::OK);

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/login/completed"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountLoginCompleted(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert_eq!(payload.login_id, Some(login_id));
    assert_eq!(payload.success, true);
    assert_eq!(payload.error, None);

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/updated"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountUpdated(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert_eq!(payload.auth_mode, Some(AuthMode::Oauth));
    assert_eq!(payload.plan_type, Some(PlanType::Pro));

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let account: GetAccountResponse = to_response(response)?;
    assert_eq!(
        account,
        GetAccountResponse {
            account: Some(Account::Oauth {
                email: Some("user@example.com".to_string()),
                // The fixture token predates the name claim.
                name: None,
                plan_type: Some(PlanType::Pro),
            }),
            requires_openai_auth: true,
        }
    );
    Ok(())
}

#[tokio::test]
async fn cancel_oauth_login_cancels_and_reports_error_completion() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    // No auth-service traffic happens before cancellation, so an unreachable
    // base URL keeps the test hermetic.
    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("FORALL_AUTH_BASE_URL", Some("http://127.0.0.1:9")),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let (login_id, _auth_url) = start_oauth_login(&mut mcp).await?;

    let cancel_id = mcp
        .send_cancel_login_account_request(CancelLoginAccountParams {
            login_id: login_id.clone(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
    )
    .await??;
    let cancel: CancelLoginAccountResponse = to_response(response)?;
    assert_eq!(cancel.status, CancelLoginAccountStatus::Canceled);

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/login/completed"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountLoginCompleted(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert_eq!(payload.login_id, Some(login_id.clone()));
    assert_eq!(payload.success, false);
    assert!(
        payload
            .error
            .as_deref()
            .is_some_and(|error| error.contains("cancelled")),
        "unexpected error: {:?}",
        payload.error
    );

    // The login is no longer pending; a second cancel is a no-op.
    let cancel_id = mcp
        .send_cancel_login_account_request(CancelLoginAccountParams { login_id })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
    )
    .await??;
    let cancel: CancelLoginAccountResponse = to_response(response)?;
    assert_eq!(cancel.status, CancelLoginAccountStatus::NotFound);
    Ok(())
}

#[tokio::test]
async fn cancel_login_with_unknown_id_returns_not_found() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path(), CreateConfigTomlParams::default())?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let cancel_id = mcp
        .send_cancel_login_account_request(CancelLoginAccountParams {
            login_id: "does-not-exist".to_string(),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(cancel_id)),
    )
    .await??;
    let cancel: CancelLoginAccountResponse = to_response(response)?;
    assert_eq!(cancel.status, CancelLoginAccountStatus::NotFound);
    Ok(())
}

#[tokio::test]
async fn logout_account_removes_auth_and_notifies() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path(), CreateConfigTomlParams::default())?;

    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    assert!(forall_home.path().join("auth.json").exists());

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let id = mcp.send_logout_account_request().await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(id)),
    )
    .await??;
    let _ok: LogoutAccountResponse = to_response(resp)?;

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/updated"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountUpdated(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert!(
        payload.auth_mode.is_none(),
        "auth_method should be None after logout"
    );
    assert_eq!(payload.plan_type, None);

    assert!(
        !forall_home.path().join("auth.json").exists(),
        "auth.json should be deleted"
    );

    let get_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    let get_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(get_id)),
    )
    .await??;
    let account: GetAccountResponse = to_response(get_resp)?;
    assert_eq!(account.account, None);
    Ok(())
}

#[tokio::test]
async fn login_account_api_key_succeeds_and_notifies() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(forall_home.path(), CreateConfigTomlParams::default())?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let req_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(req_id)),
    )
    .await??;
    let login: LoginAccountResponse = to_response(resp)?;
    assert_eq!(login, LoginAccountResponse::ApiKey {});

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/login/completed"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountLoginCompleted(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert_eq!(payload.login_id, None);
    assert_eq!(payload.success, true);
    assert_eq!(payload.error, None);

    let note = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("account/updated"),
    )
    .await??;
    let parsed: ServerNotification = note.try_into()?;
    let ServerNotification::AccountUpdated(payload) = parsed else {
        bail!("unexpected notification: {parsed:?}");
    };
    assert_eq!(payload.auth_mode, Some(AuthMode::ApiKey));
    assert_eq!(payload.plan_type, None);

    assert!(forall_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn login_account_api_key_succeeds_when_forced_chatgpt_legacy_config() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            forced_method: Some("chatgpt".to_string()),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let login: LoginAccountResponse = to_response(response)?;
    assert_eq!(login, LoginAccountResponse::ApiKey {});
    Ok(())
}

#[tokio::test]
async fn login_account_chatgpt_rejected_when_forced_api() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            forced_method: Some("api".to_string()),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_login_account_chatgpt_request().await?;
    let err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert!(
        err.error.message.contains("unknown variant `chatgpt`"),
        "unexpected error: {}",
        err.error.message
    );
    Ok(())
}

#[tokio::test]
async fn login_account_chatgpt_device_code_returns_error_when_disabled() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mock_server = MockServer::start().await;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            base_url: Some(format!("{}/v1", mock_server.uri())),
            ..Default::default()
        },
    )?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_login_account_chatgpt_device_code_request().await?;
    let err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert!(
        err.error
            .message
            .contains("unknown variant `chatgptDeviceCode`"),
        "unexpected error: {}",
        err.error.message
    );

    let maybe_completed = timeout(
        Duration::from_millis(500),
        mcp.read_stream_until_notification_message("account/login/completed"),
    )
    .await;
    assert!(
        maybe_completed.is_err(),
        "account/login/completed should not be emitted when device code start fails"
    );
    assert!(
        !forall_home.path().join("auth.json").exists(),
        "auth.json should not be created when device code start fails"
    );
    Ok(())
}

#[tokio::test]
async fn get_account_no_auth() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let account: GetAccountResponse = to_response(resp)?;

    assert_eq!(account.account, None, "expected no account");
    assert_eq!(account.requires_openai_auth, true);
    Ok(())
}

#[tokio::test]
async fn get_account_with_api_key() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let req_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(req_id)),
    )
    .await??;
    let _login_ok = to_response::<LoginAccountResponse>(resp)?;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountResponse = to_response(resp)?;

    let expected = GetAccountResponse {
        account: Some(Account::ApiKey {}),
        requires_openai_auth: true,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn get_account_when_auth_not_required() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(false),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountResponse = to_response(resp)?;

    let expected = GetAccountResponse {
        account: None,
        requires_openai_auth: false,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn get_account_with_aws_provider() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            model_provider_id: Some("amazon-bedrock".to_string()),
            extra_provider_config: Some(
                r#"[model_providers.amazon-bedrock.aws]
profile = "forall-bedrock"
region = "us-west-2"
"#
                .to_string(),
            ),
            ..Default::default()
        },
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountResponse = to_response(resp)?;

    let expected = GetAccountResponse {
        account: Some(Account::AmazonBedrock {
            credential_source: AmazonBedrockCredentialSource::AwsManaged,
        }),
        requires_openai_auth: false,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn get_account_with_managed_bedrock_provider() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            model_provider_id: Some("amazon-bedrock".to_string()),
            ..Default::default()
        },
    )?;
    login_with_bedrock_api_key(
        forall_home.path(),
        "managed-bedrock-api-key",
        "us-west-2",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountResponse = to_response(resp)?;

    assert_eq!(
        received,
        GetAccountResponse {
            account: Some(Account::AmazonBedrock {
                credential_source: AmazonBedrockCredentialSource::ForallManaged,
            }),
            requires_openai_auth: false,
        }
    );
    Ok(())
}

#[tokio::test]
async fn get_account_with_stored_api_key_fixture() -> Result<()> {
    let forall_home = TempDir::new()?;
    create_config_toml(
        forall_home.path(),
        CreateConfigTomlParams {
            requires_openai_auth: Some(true),
            ..Default::default()
        },
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("access-chatgpt"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: GetAccountResponse = to_response(resp)?;

    assert_eq!(
        received,
        GetAccountResponse {
            account: Some(Account::ApiKey {}),
            requires_openai_auth: true,
        }
    );
    Ok(())
}
