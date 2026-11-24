use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_oauth_auth;
use forall_app_server_protocol::AccountTokenUsageDailyBucket;
use forall_app_server_protocol::AccountTokenUsageSummary;
use forall_app_server_protocol::GetAccountTokenUsageResponse;
use forall_app_server_protocol::JSONRPCError;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
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

/// Usage payload as served by the gateway's `/api/account/usage` endpoint
/// (camelCase, field-for-field isomorphic to the v2 protocol type).
fn gateway_token_usage_json() -> serde_json::Value {
    serde_json::json!({
        "summary": {
            "lifetimeTokens": 1_000,
            "peakDailyTokens": 200,
            "longestRunningTurnSec": 30,
            "currentStreakDays": 2,
            "longestStreakDays": 5,
        },
        "dailyUsageBuckets": [
            {"startDate": "2026-07-28", "tokens": 42},
        ],
    })
}

fn expected_token_usage_response() -> GetAccountTokenUsageResponse {
    GetAccountTokenUsageResponse {
        summary: AccountTokenUsageSummary {
            lifetime_tokens: Some(1_000),
            peak_daily_tokens: Some(200),
            longest_running_turn_sec: Some(30),
            current_streak_days: Some(2),
            longest_streak_days: Some(5),
        },
        daily_usage_buckets: Some(vec![AccountTokenUsageDailyBucket {
            start_date: "2026-07-28".to_string(),
            tokens: 42,
        }]),
    }
}

#[tokio::test]
async fn get_account_token_usage_requires_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_token_usage_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "authentication required to read token usage"
    );

    Ok(())
}

#[tokio::test]
async fn get_account_token_usage_unavailable_for_byok_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let login_request_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;
    let _: LoginAccountResponse = to_response(
        timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(login_request_id)),
        )
        .await??,
    )?;

    let request_id = mcp.send_get_account_token_usage_request().await?;

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
async fn get_account_token_usage_returns_gateway_usage_for_oauth() -> Result<()> {
    let forall_home = TempDir::new()?;
    write_oauth_auth(forall_home.path(), "access-1", "refresh-1")?;

    let api_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/account/usage"))
        .and(header("authorization", "Bearer access-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(gateway_token_usage_json()))
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

    let request_id = mcp.send_get_account_token_usage_request().await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let usage: GetAccountTokenUsageResponse = to_response(response)?;

    assert_eq!(usage, expected_token_usage_response());
    Ok(())
}
