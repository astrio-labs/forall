use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::oauth_fixture_id_token;
use app_test_support::to_response;
use app_test_support::write_oauth_auth;
use forall_app_server_protocol::AddCreditsNudgeCreditType;
use forall_app_server_protocol::GetAccountRateLimitsResponse;
use forall_app_server_protocol::JSONRPCError;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::RateLimitReachedType;
use forall_app_server_protocol::RateLimitSnapshot;
use forall_app_server_protocol::RateLimitWindow;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::SendAddCreditsNudgeEmailParams;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const HOSTED_BACKEND_UNAVAILABLE: &str =
    "Hosted Forall backend features are not available in BYOK builds.";
const RELOGIN_REQUIRED: &str = "Forall authentication is no longer valid; please log in again.";

/// Rate-limit payload as served by the gateway's `/api/account/rate-limits`
/// endpoint (camelCase; `usedPercent` is a float on the wire).
fn gateway_rate_limit_snapshot_json() -> serde_json::Value {
    serde_json::json!({
        "limitId": "forall",
        "limitName": "forall-pro",
        "primary": {"usedPercent": 12.5, "windowDurationMins": 300, "resetsAt": 1_704_069_000},
        "secondary": {"usedPercent": 55.5, "windowDurationMins": 10_080, "resetsAt": 1_704_074_400},
        "credits": null,
        "individualLimit": null,
        "planType": null,
        "rateLimitReachedType": "rate_limit_reached",
    })
}

fn gateway_rate_limits_json() -> serde_json::Value {
    serde_json::json!({
        "rateLimits": gateway_rate_limit_snapshot_json(),
        "rateLimitsByLimitId": {"forall": gateway_rate_limit_snapshot_json()},
        "rateLimitResetCredits": null,
    })
}

fn expected_rate_limit_snapshot() -> RateLimitSnapshot {
    RateLimitSnapshot {
        limit_id: Some("forall".to_string()),
        limit_name: Some("forall-pro".to_string()),
        primary: Some(RateLimitWindow {
            used_percent: 13,
            window_duration_mins: Some(300),
            resets_at: Some(1_704_069_000),
        }),
        secondary: Some(RateLimitWindow {
            used_percent: 56,
            window_duration_mins: Some(10_080),
            resets_at: Some(1_704_074_400),
        }),
        credits: None,
        individual_limit: None,
        plan_type: None,
        rate_limit_reached_type: Some(RateLimitReachedType::RateLimitReached),
    }
}

fn expected_rate_limits_response() -> GetAccountRateLimitsResponse {
    GetAccountRateLimitsResponse {
        rate_limits: expected_rate_limit_snapshot(),
        rate_limits_by_limit_id: Some(HashMap::from([(
            "forall".to_string(),
            expected_rate_limit_snapshot(),
        )])),
        rate_limit_reset_credits: None,
    }
}

async fn login_with_api_key(mcp: &mut TestAppServer, api_key: &str) -> Result<()> {
    let request_id = mcp.send_login_account_api_key_request(api_key).await?;
    let _: LoginAccountResponse = app_test_support::to_response(
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??,
    )?;
    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_requires_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "authentication required to read rate limits"
    );

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_unavailable_for_byok_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, HOSTED_BACKEND_UNAVAILABLE);

    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_returns_gateway_snapshot_for_oauth() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_oauth_auth(forall_home.path(), "access-1", "refresh-1")?;

    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/account/rate-limits"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gateway_rate_limits_json()))
        .expect(1)
        .mount(&api_server)
        .await;

    // The env override carries the model-provider style `/v1` suffix, which the
    // account API base derivation strips.
    let api_base = format!("{}/v1", api_server.uri());
    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("FORALL_API_KEY", None),
            ("FORALL_API_BASE_URL", Some(api_base.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let rate_limits: GetAccountRateLimitsResponse = to_response(response)?;

    assert_eq!(rate_limits, expected_rate_limits_response());
    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_refreshes_token_after_unauthorized() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_oauth_auth(forall_home.path(), "access-expired", "refresh-1")?;

    let auth_server = MockServer::start().await;
    let id_token = oauth_fixture_id_token()?;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": id_token,
            "access_token": "access-fresh",
            "refresh_token": "refresh-2",
            "token_type": "bearer",
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(&auth_server)
        .await;

    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/account/rate-limits"))
        .and(header("authorization", "Bearer access-expired"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&api_server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/account/rate-limits"))
        .and(header("authorization", "Bearer access-fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gateway_rate_limits_json()))
        .expect(1)
        .mount(&api_server)
        .await;

    let auth_base = auth_server.uri();
    let api_base = format!("{}/v1", api_server.uri());
    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("FORALL_API_KEY", None),
            ("FORALL_AUTH_BASE_URL", Some(auth_base.as_str())),
            ("FORALL_API_BASE_URL", Some(api_base.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let rate_limits: GetAccountRateLimitsResponse = to_response(response)?;

    assert_eq!(rate_limits, expected_rate_limits_response());
    Ok(())
}

#[tokio::test]
async fn get_account_rate_limits_requires_relogin_when_refresh_does_not_help() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_oauth_auth(forall_home.path(), "access-expired", "refresh-1")?;

    let auth_server = MockServer::start().await;
    let id_token = oauth_fixture_id_token()?;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id_token": id_token,
            "access_token": "access-fresh",
            "refresh_token": "refresh-2",
            "token_type": "bearer",
            "expires_in": 3600,
        })))
        .expect(1)
        .mount(&auth_server)
        .await;

    // The gateway rejects both the stale and the refreshed token.
    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/account/rate-limits"))
        .respond_with(ResponseTemplate::new(401))
        .expect(2)
        .mount(&api_server)
        .await;

    let auth_base = auth_server.uri();
    let api_base = format!("{}/v1", api_server.uri());
    let mut mcp = TestAppServer::new_with_env(
        forall_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("FORALL_API_KEY", None),
            ("FORALL_AUTH_BASE_URL", Some(auth_base.as_str())),
            ("FORALL_API_BASE_URL", Some(api_base.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;
    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, RELOGIN_REQUIRED);
    Ok(())
}

#[tokio::test]
async fn send_add_credits_nudge_email_requires_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "authentication required to notify workspace owner"
    );

    Ok(())
}

#[tokio::test]
async fn send_add_credits_nudge_email_unavailable_for_byok_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, HOSTED_BACKEND_UNAVAILABLE);

    Ok(())
}
