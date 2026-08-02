use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use reqwest::header::ACCEPT;
use reqwest::header::AUTHORIZATION;
use reqwest::header::CONTENT_TYPE;
use reqwest::header::HeaderValue;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::OnceCell;

use crate::CancelVerificationResponse;
use crate::ExplainVerificationRequest;
use crate::ExplainVerificationResponse;
use crate::JobRequest;
use crate::StatusVerificationResponse;
use crate::SubmitVerificationRequest;
use crate::SubmitVerificationResponse;

pub const DEFAULT_HOSTED_MCP_ENDPOINT: &str = "https://mcp.forall.astrio.app/mcp";

#[derive(Clone)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(token: impl Into<String>) -> Result<Self, AuthError> {
        let token = token.into();
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| AuthError::InvalidToken)?;
        Ok(Self(token))
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([REDACTED])")
    }
}

pub trait BearerTokenProvider: Send + Sync {
    fn bearer_token(&self) -> Result<BearerToken, AuthError>;
}

#[derive(Clone)]
pub struct StaticBearerTokenProvider(BearerToken);

impl StaticBearerTokenProvider {
    pub fn new(token: impl Into<String>) -> Result<Self, AuthError> {
        Ok(Self(BearerToken::new(token)?))
    }
}

impl BearerTokenProvider for StaticBearerTokenProvider {
    fn bearer_token(&self) -> Result<BearerToken, AuthError> {
        Ok(self.0.clone())
    }
}

impl fmt::Debug for StaticBearerTokenProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StaticBearerTokenProvider([REDACTED])")
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum AuthError {
    #[error("bearer token contains invalid HTTP header characters")]
    InvalidToken,
    #[error("bearer token provider failed: {0}")]
    Provider(String),
}

pub struct HostedVerificationClient {
    endpoint: String,
    http: reqwest::Client,
    auth: Arc<dyn BearerTokenProvider>,
    session: OnceCell<String>,
    next_id: AtomicU64,
}

impl fmt::Debug for HostedVerificationClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedVerificationClient")
            .field("endpoint", &self.endpoint)
            .field("auth", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl HostedVerificationClient {
    pub fn new(auth: Arc<dyn BearerTokenProvider>) -> Self {
        Self::with_endpoint(DEFAULT_HOSTED_MCP_ENDPOINT, auth)
    }

    pub fn with_bearer_token(token: impl Into<String>) -> Result<Self, AuthError> {
        Ok(Self::new(Arc::new(StaticBearerTokenProvider::new(token)?)))
    }

    pub fn with_endpoint(endpoint: impl Into<String>, auth: Arc<dyn BearerTokenProvider>) -> Self {
        Self {
            endpoint: endpoint.into(),
            http: reqwest::Client::new(),
            auth,
            session: OnceCell::new(),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub async fn initialize(&self) -> Result<(), HostedVerifyError> {
        self.session
            .get_or_try_init(|| async {
                let token = self.auth.bearer_token()?;
                let request = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": self.next_request_id(),
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": {
                            "name": "forall-hosted-verify",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                });
                let response = self.post(&token, None, &request).await?;
                validate_json_rpc_response(&response.body, token.expose())?;
                let session_id = response
                    .session_id
                    .ok_or(HostedVerifyError::MissingSessionId)?;

                let initialized = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                    "params": {}
                });
                self.post(&token, Some(&session_id), &initialized).await?;
                Ok::<String, HostedVerifyError>(session_id)
            })
            .await?;
        Ok(())
    }

    pub async fn submit(
        &self,
        request: &SubmitVerificationRequest,
    ) -> Result<SubmitVerificationResponse, HostedVerifyError> {
        self.call_tool("forall_verify", request).await
    }

    pub async fn status(
        &self,
        job_id: impl Into<String>,
    ) -> Result<StatusVerificationResponse, HostedVerifyError> {
        self.call_tool(
            "forall_verification_status",
            &JobRequest {
                job_id: job_id.into(),
            },
        )
        .await
    }

    pub async fn cancel(
        &self,
        job_id: impl Into<String>,
    ) -> Result<CancelVerificationResponse, HostedVerifyError> {
        self.call_tool(
            "forall_cancel_verification",
            &JobRequest {
                job_id: job_id.into(),
            },
        )
        .await
    }

    pub async fn explain(
        &self,
        request: &ExplainVerificationRequest,
    ) -> Result<ExplainVerificationResponse, HostedVerifyError> {
        self.call_tool("forall_explain_verification", request).await
    }

    async fn call_tool<Request, Response>(
        &self,
        name: &str,
        arguments: &Request,
    ) -> Result<Response, HostedVerifyError>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        self.initialize().await?;
        let token = self.auth.bearer_token()?;
        let session = self
            .session
            .get()
            .ok_or(HostedVerifyError::MissingSessionId)?;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_request_id(),
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
        });
        let response = self.post(&token, Some(session), &request).await?;
        let result = validate_json_rpc_response(&response.body, token.expose())?;
        let payload = tool_payload(result, token.expose())?;
        serde_json::from_value(payload)
            .map_err(|error| HostedVerifyError::InvalidResponse(error.to_string()))
    }

    async fn post(
        &self,
        token: &BearerToken,
        session_id: Option<&str>,
        body: &Value,
    ) -> Result<McpResponse, HostedVerifyError> {
        ensure_secure_endpoint(&self.endpoint)?;
        let mut request = self
            .http
            .post(&self.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header(AUTHORIZATION, format!("Bearer {}", token.expose()))
            .json(body);
        if let Some(session_id) = session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        let response = request.send().await.map_err(HostedVerifyError::Transport)?;
        let status = response.status();
        let session_id = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if !status.is_success() {
            return Err(HostedVerifyError::HttpStatus(status.as_u16()));
        }
        if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
            return Ok(McpResponse {
                body: Value::Null,
                session_id,
            });
        }
        let response_body = response
            .text()
            .await
            .map_err(HostedVerifyError::Transport)?;
        let body = if content_type.starts_with("text/event-stream") {
            parse_event_stream(&response_body)?
        } else {
            serde_json::from_str(&response_body)
                .map_err(|error| HostedVerifyError::InvalidResponse(error.to_string()))?
        };
        Ok(McpResponse { body, session_id })
    }

    fn next_request_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

struct McpResponse {
    body: Value,
    session_id: Option<String>,
}

/// Refuse to attach the bearer token to a request that would leave the host in
/// cleartext. Loopback stays allowed so local servers keep working.
fn ensure_secure_endpoint(endpoint: &str) -> Result<(), HostedVerifyError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|error| HostedVerifyError::InvalidEndpoint(error.to_string()))?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        scheme => Err(HostedVerifyError::InsecureEndpoint(scheme.to_string())),
    }
}

fn validate_json_rpc_response<'a>(
    body: &'a Value,
    token: &str,
) -> Result<&'a Value, HostedVerifyError> {
    if let Some(error) = body.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("MCP request failed")
            .replace(token, "[REDACTED]");
        return Err(HostedVerifyError::Mcp { code, message });
    }
    body.get("result")
        .ok_or_else(|| HostedVerifyError::InvalidResponse("missing JSON-RPC result".to_string()))
}

fn tool_payload(result: &Value, token: &str) -> Result<Value, HostedVerifyError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let structured = result.get("structuredContent");
        let code = structured
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("tool_failed")
            .replace(token, "[REDACTED]");
        let message = structured
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("hosted verification tool reported failure")
            .replace(token, "[REDACTED]");
        return Err(HostedVerifyError::ToolFailed { code, message });
    }
    if let Some(structured) = result.get("structuredContent") {
        return Ok(structured.clone());
    }
    if let Some(text) = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| content.first())
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
    {
        return serde_json::from_str(text)
            .map_err(|error| HostedVerifyError::InvalidResponse(error.to_string()));
    }
    Ok(result.clone())
}

fn parse_event_stream(body: &str) -> Result<Value, HostedVerifyError> {
    for event in body.split("\n\n") {
        let data = event
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<String>();
        if data.is_empty() {
            continue;
        }
        return serde_json::from_str(&data)
            .map_err(|error| HostedVerifyError::InvalidResponse(error.to_string()));
    }
    Err(HostedVerifyError::InvalidResponse(
        "event stream contained no JSON-RPC data".to_string(),
    ))
}

#[derive(Debug, thiserror::Error)]
pub enum HostedVerifyError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error("hosted MCP transport failed: {0}")]
    Transport(reqwest::Error),
    #[error("hosted MCP returned HTTP {0}")]
    HttpStatus(u16),
    #[error("hosted MCP endpoint is not a valid URL: {0}")]
    InvalidEndpoint(String),
    #[error(
        "hosted MCP endpoint uses {0}; https is required so the bearer token is not sent in cleartext"
    )]
    InsecureEndpoint(String),
    #[error("hosted MCP initialize response omitted Mcp-Session-Id")]
    MissingSessionId,
    #[error("hosted MCP error {code}: {message}")]
    Mcp { code: i64, message: String },
    #[error("hosted MCP tool failed ({code}): {message}")]
    ToolFailed { code: String, message: String },
    #[error("invalid hosted MCP response: {0}")]
    InvalidResponse(String),
}
