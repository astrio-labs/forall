use anyhow::Result;
use app_test_support::TestAppServer;
use forall_app_server_protocol::ConsumeAccountRateLimitResetCreditParams;
use forall_app_server_protocol::JSONRPCError;
use forall_app_server_protocol::LoginAccountResponse;
use forall_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;
const HOSTED_BACKEND_UNAVAILABLE: &str =
    "Hosted Forall backend features are not available in BYOK builds.";

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
async fn consume_rate_limit_reset_credit_requires_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(forall_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            ConsumeAccountRateLimitResetCreditParams {
                idempotency_key: "reset-1".to_string(),
            },
        )
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "authentication required for rate limit reset credits"
    );

    Ok(())
}

#[tokio::test]
async fn consume_rate_limit_reset_credit_unavailable_for_byok_auth() -> Result<()> {
    let forall_home = TempDir::new()?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            ConsumeAccountRateLimitResetCreditParams {
                idempotency_key: "reset-1".to_string(),
            },
        )
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
