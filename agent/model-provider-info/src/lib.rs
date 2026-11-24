//! Registry of model providers supported by Forall.
//!
//! Providers can be defined in two places:
//!   1. Built-in defaults compiled into the binary so Forall works out-of-the-box.
//!   2. User-defined entries inside `~/.forall/config.toml` under the `model_providers`
//!      key. These override or extend the defaults at runtime.

use forall_api::Provider as ApiProvider;
use forall_api::RetryConfig as ApiRetryConfig;
use forall_api::is_azure_responses_provider;
use forall_app_server_protocol::AuthMode;
use forall_protocol::config_types::ModelProviderAuthInfo;
use forall_protocol::error::EnvVarError;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result as ForallResult;
use http::HeaderMap;
use http::header::HeaderName;
use http::header::HeaderValue;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_STREAM_MAX_RETRIES: u64 = 5;
const DEFAULT_REQUEST_MAX_RETRIES: u64 = 4;
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Hard cap for user-configured `stream_max_retries`.
const MAX_STREAM_MAX_RETRIES: u64 = 100;
/// Hard cap for user-configured `request_max_retries`.
const MAX_REQUEST_MAX_RETRIES: u64 = 100;

const OPENAI_PROVIDER_NAME: &str = "OpenAI";
pub const OPENAI_PROVIDER_ID: &str = "openai";
const AMAZON_BEDROCK_PROVIDER_NAME: &str = "Amazon Bedrock";
pub const AMAZON_BEDROCK_PROVIDER_ID: &str = "amazon-bedrock";
pub const AMAZON_BEDROCK_GPT_5_5_MODEL_ID: &str = "openai.gpt-5.5";
pub const AMAZON_BEDROCK_GPT_5_4_MODEL_ID: &str = "openai.gpt-5.4";
pub const AMAZON_BEDROCK_DEFAULT_BASE_URL: &str =
    "https://bedrock-mantle.us-east-1.api.aws/openai/v1";
const AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER: &str = "x-amzn-mantle-client-agent";
const AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE: &str = "codex";
const BEDROCK_ANTHROPIC_PROVIDER_NAME: &str = "Amazon Bedrock (Anthropic)";
pub const BEDROCK_ANTHROPIC_PROVIDER_ID: &str = "bedrock-anthropic";
/// Flagship bundled Claude slug served by the Bedrock Anthropic provider.
///
/// Bedrock identifies models as `{vendor}.{model}`, matching the convention the
/// Mantle catalog already uses (`openai.gpt-5.5`).
pub const BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID: &str = "anthropic.claude-opus-4-6";
/// Default `bedrock-runtime` endpoint, replaced at request time by the endpoint
/// for the resolved region.
pub const BEDROCK_ANTHROPIC_DEFAULT_BASE_URL: &str =
    "https://bedrock-runtime.us-east-1.amazonaws.com";
const ASTRIO_PROVIDER_NAME: &str = "Forall";
pub const ASTRIO_PROVIDER_ID: &str = "astrio";
pub const ASTRIO_DEFAULT_BASE_URL: &str = "https://api.forall.astrio.app/v1";
pub const ASTRIO_BASE_URL_ENV_VAR: &str = "FORALL_API_BASE_URL";
const AZURE_PROVIDER_NAME: &str = "Azure OpenAI";
pub const AZURE_PROVIDER_ID: &str = "azure";
pub const AZURE_OPENAI_BASE_URL_ENV_VAR: &str = "AZURE_OPENAI_BASE_URL";
pub const AZURE_OPENAI_API_KEY_ENV_VAR: &str = "AZURE_OPENAI_API_KEY";
pub const AZURE_OPENAI_API_VERSION_ENV_VAR: &str = "AZURE_OPENAI_API_VERSION";
const AZURE_OPENAI_DEFAULT_API_VERSION: &str = "preview";
const AZURE_AUTH_HEADER: &str = "api-key";
const ANTHROPIC_PROVIDER_NAME: &str = "Anthropic";
pub const ANTHROPIC_PROVIDER_ID: &str = "anthropic";
pub const ANTHROPIC_DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";
pub const ANTHROPIC_BASE_URL_ENV_VAR: &str = "ANTHROPIC_BASE_URL";
pub const ANTHROPIC_API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";
/// Flagship bundled Claude model slug served by the Anthropic provider.
pub const ANTHROPIC_CLAUDE_OPUS_MODEL_ID: &str = "claude-opus-4-6";
/// Anthropic sends the API key verbatim in `x-api-key` instead of a bearer token.
const ANTHROPIC_AUTH_HEADER: &str = "x-api-key";
const ANTHROPIC_VERSION_HEADER: &str = "anthropic-version";
const ANTHROPIC_VERSION_VALUE: &str = "2023-06-01";
const GEMINI_PROVIDER_NAME: &str = "Google Gemini";
pub const GEMINI_PROVIDER_ID: &str = "gemini";
/// The base URL carries the API version segment; endpoint paths are appended as
/// `models/{model}:streamGenerateContent`.
pub const GEMINI_DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
pub const GEMINI_BASE_URL_ENV_VAR: &str = "GEMINI_BASE_URL";
pub const GEMINI_API_KEY_ENV_VAR: &str = "GEMINI_API_KEY";
/// Flagship bundled Gemini model slug served by the Gemini provider.
pub const GEMINI_PRO_MODEL_ID: &str = "gemini-2.5-pro";
/// Gemini sends the API key verbatim in `x-goog-api-key` instead of a bearer
/// token. (The `?key=` query-parameter form is deliberately not used: it would
/// put the credential in the request URL.)
const GEMINI_AUTH_HEADER: &str = "x-goog-api-key";
const OPENROUTER_PROVIDER_NAME: &str = "OpenRouter";
pub const OPENROUTER_PROVIDER_ID: &str = "openrouter";
pub const OPENROUTER_DEFAULT_MODEL: &str = "openrouter/auto";
const OPENROUTER_DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_API_KEY_ENV: &str = "OPENROUTER_API_KEY";
const LITELLM_PROVIDER_NAME: &str = "LiteLLM";
pub const LITELLM_PROVIDER_ID: &str = "litellm";
pub const DEFAULT_LITELLM_PORT: u16 = 4000;
const CHAT_WIRE_API_REMOVED_ERROR: &str = "`wire_api = \"chat\"` is no longer supported.\nHow to fix: set `wire_api = \"responses\"` in your provider config.\nMore info: https://github.com/astrio-labs/forall";
pub const LEGACY_OLLAMA_CHAT_PROVIDER_ID: &str = "ollama-chat";
pub const OLLAMA_CHAT_PROVIDER_REMOVED_ERROR: &str = "`ollama-chat` is no longer supported.\nHow to fix: replace `ollama-chat` with `ollama` in `model_provider`, `oss_provider`, or `--local-provider`.\nMore info: https://github.com/astrio-labs/forall";

/// Wire protocol that the provider speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    /// The Responses API exposed by OpenAI at `/v1/responses`.
    #[default]
    Responses,
    /// The Anthropic Messages API exposed at `/v1/messages`.
    Messages,
    /// The native Google Gemini API exposed at
    /// `/v1beta/models/{model}:streamGenerateContent`.
    ///
    /// This is the native `generateContent` protocol, not Google's
    /// OpenAI-compatibility shim (which speaks Chat Completions, a wire
    /// protocol this repo deliberately removed).
    Gemini,
    /// The Anthropic Messages schema carried over Amazon Bedrock's
    /// `/model/{modelId}/invoke-with-response-stream`.
    ///
    /// Distinct from [`Self::Messages`] even though the request body and the
    /// streaming chunks are the same: the model is URL-path scoped rather than a
    /// body field, the API version travels in the body rather than a header, and
    /// the response is `application/vnd.amazon.eventstream` (binary frames)
    /// rather than SSE.
    // Explicitly renamed: `rename_all = "lowercase"` would emit
    // `bedrockmessages`, which the hand-written `Deserialize` below rejects.
    #[serde(rename = "bedrock_messages")]
    BedrockMessages,
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Responses => "responses",
            Self::Messages => "messages",
            Self::Gemini => "gemini",
            Self::BedrockMessages => "bedrock_messages",
        };
        f.write_str(value)
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "messages" => Ok(Self::Messages),
            "gemini" => Ok(Self::Gemini),
            "bedrock_messages" => Ok(Self::BedrockMessages),
            "chat" => Err(serde::de::Error::custom(CHAT_WIRE_API_REMOVED_ERROR)),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["responses", "messages", "gemini", "bedrock_messages"],
            )),
        }
    }
}

/// Serializable representation of a provider definition.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderInfo {
    /// Friendly display name.
    #[serde(default)]
    pub name: String,
    /// Base URL for the provider's OpenAI-compatible API.
    pub base_url: Option<String>,
    /// Environment variable that stores the user's API key for this provider.
    pub env_key: Option<String>,

    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub env_key_instructions: Option<String>,
    /// Value to use with `Authorization: Bearer <token>` header. Use of this
    /// config is discouraged in favor of `env_key` for security reasons, but
    /// this may be necessary when using this programmatically.
    pub experimental_bearer_token: Option<String>,
    /// Command-backed bearer-token configuration for this provider.
    pub auth: Option<ModelProviderAuthInfo>,
    /// AWS SigV4 auth configuration for this provider.
    pub aws: Option<ModelProviderAwsAuthInfo>,
    /// HTTP header that carries the provider API key. `None` sends the
    /// standard `Authorization: Bearer <key>` header; providers such as Azure
    /// OpenAI instead expect the key verbatim in a named header (`api-key`).
    #[serde(default)]
    pub auth_header: Option<String>,
    /// Which wire protocol this provider expects.
    #[serde(default)]
    pub wire_api: WireApi,
    /// Optional query parameters to append to the base URL.
    pub query_params: Option<HashMap<String, String>>,
    /// Additional HTTP headers to include in requests to this provider where
    /// the (key, value) pairs are the header name and value.
    pub http_headers: Option<HashMap<String, String>>,
    /// Optional HTTP headers to include in requests to this provider where the
    /// (key, value) pairs are the header name and _environment variable_ whose
    /// value should be used. If the environment variable is not set, or the
    /// value is empty, the header will not be included in the request.
    pub env_http_headers: Option<HashMap<String, String>>,
    /// Maximum number of times to retry a failed HTTP request to this provider.
    pub request_max_retries: Option<u64>,
    /// Number of times to retry reconnecting a dropped streaming response before failing.
    pub stream_max_retries: Option<u64>,
    /// Idle timeout (in milliseconds) to wait for activity on a streaming response before treating
    /// the connection as lost.
    pub stream_idle_timeout_ms: Option<u64>,
    /// Maximum time (in milliseconds) to wait for a websocket connection attempt before treating
    /// it as failed.
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Does this provider require an OpenAI API Key or ChatGPT login token? If true,
    /// user is presented with login screen on first run, and login preference and token/key
    /// are stored in auth.json. If false (which is the default), login screen is skipped,
    /// and API key (if needed) comes from the "env_key" environment variable.
    #[serde(default)]
    pub requires_openai_auth: bool,
    /// Does this provider authenticate with the user's Forall account (OAuth
    /// tokens obtained via `forall login`)? When true, requests attach the
    /// Forall access token as a bearer token and never use provider API keys.
    #[serde(default)]
    pub requires_forall_auth: bool,
    /// Whether this provider supports the Responses API WebSocket transport.
    #[serde(default)]
    pub supports_websockets: bool,
}

/// AWS SigV4 auth configuration for a model provider.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderAwsAuthInfo {
    /// AWS profile name to use. When unset, the AWS SDK default chain decides.
    pub profile: Option<String>,
    /// AWS region to use for provider-specific endpoints.
    pub region: Option<String>,
}

impl ModelProviderInfo {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if let Some(auth_header) = self.auth_header.as_deref() {
            if HeaderName::try_from(auth_header).is_err() {
                return Err(format!(
                    "provider auth_header is not a valid HTTP header name: `{auth_header}`"
                ));
            }
            if self.requires_forall_auth {
                return Err(
                    "provider auth_header cannot be combined with requires_forall_auth".to_string(),
                );
            }
        }

        if self.aws.is_some() {
            if self.supports_websockets {
                // TODO(celia-oai): Support AWS SigV4 signing for WebSocket
                // upgrade requests before allowing AWS-authenticated providers
                // to enable Responses-over-WebSocket.
                return Err("provider aws cannot be combined with supports_websockets".to_string());
            }

            let mut conflicts = Vec::new();
            if self.env_key.is_some() {
                conflicts.push("env_key");
            }
            if self.experimental_bearer_token.is_some() {
                conflicts.push("experimental_bearer_token");
            }
            if self.auth.is_some() {
                conflicts.push("auth");
            }
            if self.requires_openai_auth {
                conflicts.push("requires_openai_auth");
            }
            if self.auth_header.is_some() {
                conflicts.push("auth_header");
            }

            if !conflicts.is_empty() {
                return Err(format!(
                    "provider aws cannot be combined with {}",
                    conflicts.join(", ")
                ));
            }
        }

        let Some(auth) = self.auth.as_ref() else {
            return Ok(());
        };

        if auth.command.trim().is_empty() {
            return Err("provider auth.command must not be empty".to_string());
        }

        let mut conflicts = Vec::new();
        if self.env_key.is_some() {
            conflicts.push("env_key");
        }
        if self.experimental_bearer_token.is_some() {
            conflicts.push("experimental_bearer_token");
        }
        if self.requires_openai_auth {
            conflicts.push("requires_openai_auth");
        }

        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "provider auth cannot be combined with {}",
                conflicts.join(", ")
            ))
        }
    }

    fn build_header_map(&self) -> ForallResult<HeaderMap> {
        let capacity = self.http_headers.as_ref().map_or(0, HashMap::len)
            + self.env_http_headers.as_ref().map_or(0, HashMap::len);
        let mut headers = HeaderMap::with_capacity(capacity);
        if let Some(extra) = &self.http_headers {
            for (k, v) in extra {
                if let (Ok(name), Ok(value)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                    headers.insert(name, value);
                }
            }
        }

        if let Some(env_headers) = &self.env_http_headers {
            for (header, env_var) in env_headers {
                if let Ok(val) = std::env::var(env_var)
                    && !val.trim().is_empty()
                    && let (Ok(name), Ok(value)) =
                        (HeaderName::try_from(header), HeaderValue::try_from(val))
                {
                    headers.insert(name, value);
                }
            }
        }

        Ok(headers)
    }

    pub fn to_api_provider(&self, auth_mode: Option<AuthMode>) -> ForallResult<ApiProvider> {
        let _ = auth_mode;
        let default_base_url = "https://api.openai.com/v1";
        let base_url = match self.base_url.clone() {
            Some(base_url) => base_url,
            // Azure has no usable default endpoint: falling back to the OpenAI
            // base URL would silently send the Azure API key to the wrong host,
            // so reject at use-time until AZURE_OPENAI_BASE_URL is set.
            None if is_azure_responses_provider(&self.name, /*base_url*/ None) => {
                return Err(ForallErr::EnvVar(EnvVarError {
                    var: AZURE_OPENAI_BASE_URL_ENV_VAR.to_string(),
                    instructions: Some(format!(
                        "Set {AZURE_OPENAI_BASE_URL_ENV_VAR} to your Azure OpenAI resource's \
OpenAI endpoint, e.g. https://<resource>.openai.azure.com/openai/v1."
                    )),
                }));
            }
            None => default_base_url.to_string(),
        };

        let headers = self.build_header_map()?;
        let retry = ApiRetryConfig {
            max_attempts: self.request_max_retries(),
            base_delay: Duration::from_millis(200),
            retry_429: false,
            retry_5xx: true,
            retry_transport: true,
        };

        Ok(ApiProvider {
            name: self.name.clone(),
            base_url,
            query_params: self.query_params.clone(),
            headers,
            retry,
            stream_idle_timeout: self.stream_idle_timeout(),
        })
    }

    /// If `env_key` is Some, returns the API key for this provider if present
    /// (and non-empty) in the environment. If `env_key` is required but
    /// cannot be found, returns an error.
    pub fn api_key(&self) -> ForallResult<Option<String>> {
        match &self.env_key {
            Some(env_key) => {
                let api_key = std::env::var(env_key)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        ForallErr::EnvVar(EnvVarError {
                            var: env_key.clone(),
                            instructions: self.env_key_instructions.clone(),
                        })
                    })?;
                Ok(Some(api_key))
            }
            None => Ok(None),
        }
    }

    /// Effective maximum number of request retries for this provider.
    pub fn request_max_retries(&self) -> u64 {
        self.request_max_retries
            .unwrap_or(DEFAULT_REQUEST_MAX_RETRIES)
            .min(MAX_REQUEST_MAX_RETRIES)
    }

    /// Effective maximum number of stream reconnection attempts for this provider.
    pub fn stream_max_retries(&self) -> u64 {
        self.stream_max_retries
            .unwrap_or(DEFAULT_STREAM_MAX_RETRIES)
            .min(MAX_STREAM_MAX_RETRIES)
    }

    /// Effective idle timeout for streaming responses.
    pub fn stream_idle_timeout(&self) -> Duration {
        self.stream_idle_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_STREAM_IDLE_TIMEOUT_MS))
    }

    /// Effective timeout for websocket connect attempts.
    pub fn websocket_connect_timeout(&self) -> Duration {
        self.websocket_connect_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS))
    }

    pub fn create_openai_provider(base_url: Option<String>) -> ModelProviderInfo {
        ModelProviderInfo {
            name: OPENAI_PROVIDER_NAME.into(),
            base_url,
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(
                [("version".to_string(), env!("CARGO_PKG_VERSION").to_string())]
                    .into_iter()
                    .collect(),
            ),
            env_http_headers: Some(
                [
                    (
                        "OpenAI-Organization".to_string(),
                        "OPENAI_ORGANIZATION".to_string(),
                    ),
                    ("OpenAI-Project".to_string(), "OPENAI_PROJECT".to_string()),
                ]
                .into_iter()
                .collect(),
            ),
            // Use global defaults for retry/timeout unless overridden in config.toml.
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: true,
            requires_forall_auth: false,
            supports_websockets: true,
        }
    }

    pub fn create_amazon_bedrock_provider(
        aws: Option<ModelProviderAwsAuthInfo>,
    ) -> ModelProviderInfo {
        ModelProviderInfo {
            name: AMAZON_BEDROCK_PROVIDER_NAME.into(),
            base_url: Some(AMAZON_BEDROCK_DEFAULT_BASE_URL.into()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(aws.unwrap_or(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            })),
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(HashMap::from([(
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string(),
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    /// Built-in provider for Claude models on Amazon Bedrock.
    ///
    /// Deliberately a *separate* provider from `amazon-bedrock` rather than a
    /// per-model route inside it: the two share nothing at request time. This
    /// one talks to `bedrock-runtime` (SigV4 service `bedrock`) and speaks
    /// [`WireApi::BedrockMessages`]; `amazon-bedrock` talks to the
    /// OpenAI-compatible Mantle front door (SigV4 service `bedrock-mantle`) and
    /// speaks [`WireApi::Responses`]. Splitting them keeps the existing
    /// GPT-on-Bedrock path — including `merge_configured_model_providers`'s
    /// `amazon-bedrock` special case — untouched.
    ///
    /// The base URL is a placeholder for the default region; the runtime
    /// provider replaces it with the endpoint for the resolved region.
    pub fn create_bedrock_anthropic_provider(
        aws: Option<ModelProviderAwsAuthInfo>,
    ) -> ModelProviderInfo {
        ModelProviderInfo {
            name: BEDROCK_ANTHROPIC_PROVIDER_NAME.into(),
            base_url: Some(BEDROCK_ANTHROPIC_DEFAULT_BASE_URL.into()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(aws.unwrap_or(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            })),
            auth_header: None,
            wire_api: WireApi::BedrockMessages,
            query_params: None,
            // No `anthropic-version` header: Bedrock takes the API version in
            // the request body instead (`anthropic_version`), and every header
            // sent is covered by the SigV4 signature.
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    pub fn is_openai(&self) -> bool {
        self.name == OPENAI_PROVIDER_NAME
    }

    pub fn is_amazon_bedrock(&self) -> bool {
        self.name == AMAZON_BEDROCK_PROVIDER_NAME
    }

    pub fn is_bedrock_anthropic(&self) -> bool {
        self.name == BEDROCK_ANTHROPIC_PROVIDER_NAME
    }

    /// Built-in Forall provider backed by the user's Forall account (OAuth).
    pub fn create_astrio_provider() -> ModelProviderInfo {
        let base_url = std::env::var(ASTRIO_BASE_URL_ENV_VAR)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| ASTRIO_DEFAULT_BASE_URL.to_string());

        ModelProviderInfo {
            name: ASTRIO_PROVIDER_NAME.into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: true,
            supports_websockets: false,
        }
    }

    /// Built-in Azure OpenAI provider (BYOK).
    ///
    /// `AZURE_OPENAI_BASE_URL` must point at the Azure OpenAI resource's
    /// OpenAI endpoint, e.g. `https://<resource>.openai.azure.com/openai/v1`.
    /// The provider is always registered; when the env var is unset,
    /// `base_url` stays `None` and [`Self::to_api_provider`] rejects the
    /// provider at use-time instead of silently falling back to the OpenAI
    /// base URL. The API key from `AZURE_OPENAI_API_KEY` is sent in the
    /// Azure-specific `api-key` header rather than `Authorization: Bearer`.
    pub fn create_azure_provider() -> ModelProviderInfo {
        let base_url = std::env::var(AZURE_OPENAI_BASE_URL_ENV_VAR)
            .ok()
            .filter(|value| !value.trim().is_empty());
        let api_version = std::env::var(AZURE_OPENAI_API_VERSION_ENV_VAR)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| AZURE_OPENAI_DEFAULT_API_VERSION.to_string());

        ModelProviderInfo {
            name: AZURE_PROVIDER_NAME.into(),
            base_url,
            env_key: Some(AZURE_OPENAI_API_KEY_ENV_VAR.into()),
            env_key_instructions: Some(format!(
                "Export {AZURE_OPENAI_API_KEY_ENV_VAR} with an API key for your Azure OpenAI \
resource and set {AZURE_OPENAI_BASE_URL_ENV_VAR} to the resource's OpenAI endpoint, \
e.g. https://<resource>.openai.azure.com/openai/v1."
            )),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: Some(AZURE_AUTH_HEADER.into()),
            wire_api: WireApi::Responses,
            query_params: Some(HashMap::from([("api-version".to_string(), api_version)])),
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    /// Built-in Anthropic provider (BYOK) speaking the Messages wire protocol.
    ///
    /// The API key from `ANTHROPIC_API_KEY` is sent verbatim in the
    /// Anthropic-specific `x-api-key` header rather than
    /// `Authorization: Bearer`, and every request carries the pinned
    /// `anthropic-version` header. `ANTHROPIC_BASE_URL` overrides the default
    /// `https://api.anthropic.com/v1` endpoint.
    pub fn create_anthropic_provider() -> ModelProviderInfo {
        let base_url = std::env::var(ANTHROPIC_BASE_URL_ENV_VAR)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| ANTHROPIC_DEFAULT_BASE_URL.to_string());

        ModelProviderInfo {
            name: ANTHROPIC_PROVIDER_NAME.into(),
            base_url: Some(base_url),
            env_key: Some(ANTHROPIC_API_KEY_ENV_VAR.into()),
            env_key_instructions: Some(
                "Create an API key at https://platform.claude.com/ and export ANTHROPIC_API_KEY."
                    .into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: Some(ANTHROPIC_AUTH_HEADER.into()),
            wire_api: WireApi::Messages,
            query_params: None,
            http_headers: Some(HashMap::from([(
                ANTHROPIC_VERSION_HEADER.to_string(),
                ANTHROPIC_VERSION_VALUE.to_string(),
            )])),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    pub fn is_anthropic(&self) -> bool {
        self.name == ANTHROPIC_PROVIDER_NAME
    }

    /// Built-in Google Gemini provider (BYOK) speaking the native Gemini
    /// `generateContent` wire protocol.
    ///
    /// The API key from `GEMINI_API_KEY` is sent verbatim in the
    /// Google-specific `x-goog-api-key` header rather than
    /// `Authorization: Bearer`. `GEMINI_BASE_URL` overrides the default
    /// `https://generativelanguage.googleapis.com/v1beta` endpoint; note the
    /// base URL includes the API version segment, because Gemini's paths are
    /// model-scoped (`models/{model}:streamGenerateContent`) and carry no
    /// version of their own.
    pub fn create_gemini_provider() -> ModelProviderInfo {
        let base_url = std::env::var(GEMINI_BASE_URL_ENV_VAR)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| GEMINI_DEFAULT_BASE_URL.to_string());

        ModelProviderInfo {
            name: GEMINI_PROVIDER_NAME.into(),
            base_url: Some(base_url),
            env_key: Some(GEMINI_API_KEY_ENV_VAR.into()),
            env_key_instructions: Some(
                "Create an API key at https://aistudio.google.com/apikey and export GEMINI_API_KEY."
                    .into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: Some(GEMINI_AUTH_HEADER.into()),
            wire_api: WireApi::Gemini,
            // `alt=sse` is appended by the endpoint rather than configured here,
            // so a user-supplied `query_params` cannot displace it.
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    pub fn is_gemini(&self) -> bool {
        self.name == GEMINI_PROVIDER_NAME
    }

    pub fn create_openrouter_provider() -> ModelProviderInfo {
        ModelProviderInfo {
            name: OPENROUTER_PROVIDER_NAME.into(),
            base_url: Some(OPENROUTER_DEFAULT_BASE_URL.into()),
            env_key: Some(OPENROUTER_API_KEY_ENV.into()),
            env_key_instructions: Some(
                "Create an API key at https://openrouter.ai/settings/keys and export OPENROUTER_API_KEY."
                    .into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    pub fn create_litellm_provider() -> ModelProviderInfo {
        let default_base_url = format!(
            "http://localhost:{litellm_port}/v1",
            litellm_port = std::env::var("LITELLM_PORT")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_LITELLM_PORT)
        );
        let base_url = std::env::var("LITELLM_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_base_url);

        ModelProviderInfo {
            name: LITELLM_PROVIDER_NAME.into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: Some(
                "Optional. Export LITELLM_API_KEY when your proxy requires auth, or pipe a key to `forall login --with-api-key`."
                    .into(),
            ),
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    }

    pub fn supports_remote_compaction(&self) -> bool {
        self.is_openai() || is_azure_responses_provider(&self.name, self.base_url.as_deref())
    }

    pub fn has_command_auth(&self) -> bool {
        self.auth.is_some()
    }

    /// Whether this provider owns its model catalog via an OpenAI-compatible `/models` endpoint.
    ///
    /// When true, Forall fetches the remote catalog instead of merging with bundled GPT models.
    pub fn uses_provider_model_catalog(&self) -> bool {
        if self.requires_openai_auth
            || self.is_openai()
            || self.is_amazon_bedrock()
            || self.is_bedrock_anthropic()
        {
            return false;
        }
        if self.aws.is_some() || self.auth.is_some() {
            return false;
        }
        if self.name == "gpt-oss" {
            return false;
        }
        // Only OpenAI-compatible Responses providers expose a `/models`
        // catalog endpoint we can consume; Messages (Anthropic) and Gemini
        // providers ship bundled catalogs instead.
        self.base_url.is_some() && self.wire_api == WireApi::Responses
    }
}

/// Built-in `--profile` names that map directly to a model provider without a profile config file.
pub fn builtin_provider_profile_id(profile: &str) -> Option<String> {
    match profile {
        OPENROUTER_PROVIDER_ID => Some(OPENROUTER_PROVIDER_ID.to_string()),
        LITELLM_PROVIDER_ID => Some(LITELLM_PROVIDER_ID.to_string()),
        ASTRIO_PROVIDER_ID => Some(ASTRIO_PROVIDER_ID.to_string()),
        _ => None,
    }
}

/// Infer a BYOK model provider from environment variables when config does not set one.
///
/// Priority mirrors common agent tooling: prefer OpenAI when configured, then OpenRouter,
/// then an explicit LiteLLM proxy URL.
pub fn infer_model_provider_from_env() -> Option<String> {
    let openai_api_key = env_var_non_empty("OPENAI_API_KEY");
    let openrouter_api_key = env_var_non_empty(OPENROUTER_API_KEY_ENV);
    let litellm_base_url = env_var_non_empty("LITELLM_BASE_URL");

    if openai_api_key.is_some() {
        return None;
    }
    if openrouter_api_key.is_some() {
        return Some(OPENROUTER_PROVIDER_ID.to_string());
    }
    if litellm_base_url.is_some() {
        return Some(LITELLM_PROVIDER_ID.to_string());
    }
    None
}

/// Resolve the effective model provider id from overrides, config, profile, and environment.
///
/// Environment inference (`OPENROUTER_API_KEY`, `LITELLM_BASE_URL`, etc.) only runs when
/// `allow_env_inference` is true. Callers should disable it until the user completes BYOK
/// provider onboarding (`byok_provider` in config.toml).
pub fn resolve_effective_model_provider_id(
    config_provider: Option<&str>,
    override_provider: Option<&str>,
    profile_v2: Option<&str>,
    allow_env_inference: bool,
) -> String {
    override_provider
        .map(str::to_string)
        .or_else(|| config_provider.map(str::to_string))
        .or_else(|| profile_v2.and_then(builtin_provider_profile_id))
        .or_else(|| {
            if allow_env_inference {
                infer_model_provider_from_env()
            } else {
                None
            }
        })
        .unwrap_or_else(|| OPENAI_PROVIDER_ID.to_string())
}

fn env_var_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub const DEFAULT_LMSTUDIO_PORT: u16 = 1234;
pub const DEFAULT_OLLAMA_PORT: u16 = 11434;

pub const LMSTUDIO_OSS_PROVIDER_ID: &str = "lmstudio";
pub const OLLAMA_OSS_PROVIDER_ID: &str = "ollama";

/// Built-in default provider list.
pub fn built_in_model_providers(
    openai_base_url: Option<String>,
) -> HashMap<String, ModelProviderInfo> {
    use ModelProviderInfo as P;
    let openai_provider = P::create_openai_provider(openai_base_url);
    let amazon_bedrock_provider = P::create_amazon_bedrock_provider(/*aws*/ None);

    // Built-in providers: OpenAI, Bedrock, local OSS, and BYOK gateways (OpenRouter,
    // LiteLLM). Users can still extend `model_providers` in config.toml for custom
    // OpenAI-compatible endpoints.
    [
        (OPENAI_PROVIDER_ID, openai_provider),
        (AMAZON_BEDROCK_PROVIDER_ID, amazon_bedrock_provider),
        (
            BEDROCK_ANTHROPIC_PROVIDER_ID,
            P::create_bedrock_anthropic_provider(/*aws*/ None),
        ),
        (ASTRIO_PROVIDER_ID, P::create_astrio_provider()),
        (AZURE_PROVIDER_ID, P::create_azure_provider()),
        (ANTHROPIC_PROVIDER_ID, P::create_anthropic_provider()),
        (GEMINI_PROVIDER_ID, P::create_gemini_provider()),
        (OPENROUTER_PROVIDER_ID, P::create_openrouter_provider()),
        (LITELLM_PROVIDER_ID, P::create_litellm_provider()),
        (
            OLLAMA_OSS_PROVIDER_ID,
            create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::Responses),
        ),
        (
            LMSTUDIO_OSS_PROVIDER_ID,
            create_oss_provider(DEFAULT_LMSTUDIO_PORT, WireApi::Responses),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Merge configured providers into the built-in provider catalog.
///
/// Configured providers extend the built-in set. Built-in providers are not
/// generally overridable, but the two built-in AWS providers
/// (`amazon-bedrock`, `bedrock-anthropic`) allow the user to set `aws.profile`
/// and `aws.region`.
pub fn merge_configured_model_providers(
    mut model_providers: HashMap<String, ModelProviderInfo>,
    configured_model_providers: HashMap<String, ModelProviderInfo>,
) -> Result<HashMap<String, ModelProviderInfo>, String> {
    for (key, provider) in configured_model_providers {
        if key == AMAZON_BEDROCK_PROVIDER_ID || key == BEDROCK_ANTHROPIC_PROVIDER_ID {
            apply_aws_only_override(&mut model_providers, &key, provider)?;
        } else if key == AZURE_PROVIDER_ID
            && let Some(built_in) = model_providers.get_mut(AZURE_PROVIDER_ID)
        {
            // Azure's built-in entry only derives its endpoint from the
            // environment; the deployment-specific settings live in
            // config.toml. Plain `or_insert` would discard the user's entry
            // entirely (the built-in already occupies the key), so overlay
            // their explicitly-set fields onto the built-in instead. This keeps
            // `auth_header`/`env_key`/`api-version` working when the user only
            // sets `base_url`, while still letting them override any of them.
            overlay_configured_provider(built_in, provider);
        } else {
            model_providers.entry(key).or_insert(provider);
        }
    }

    Ok(model_providers)
}

/// Applies an `aws.profile`/`aws.region` override to a built-in AWS provider,
/// rejecting every other configured field.
///
/// Both AWS providers resolve their endpoint, credentials, and wire protocol at
/// request time from the AWS config chain, so accepting a partial override of
/// (say) `base_url` or `env_key` would silently produce a provider that cannot
/// work. The error text names the provider the user actually configured.
fn apply_aws_only_override(
    model_providers: &mut HashMap<String, ModelProviderInfo>,
    key: &str,
    mut provider: ModelProviderInfo,
) -> Result<(), String> {
    let aws_override = provider.aws.take();
    if provider != ModelProviderInfo::default() {
        return Err(format!(
            "model_providers.{key} only supports changing \
`aws.profile` and `aws.region`; other non-default provider fields are not supported"
        ));
    }

    if let Some(aws_override) = aws_override
        && let Some(built_in_provider) = model_providers.get_mut(key)
        && let Some(built_in_aws) = built_in_provider.aws.as_mut()
    {
        if let Some(profile) = aws_override.profile {
            built_in_aws.profile = Some(profile);
        }
        if let Some(region) = aws_override.region {
            built_in_aws.region = Some(region);
        }
    }

    Ok(())
}

/// Overlays a user-configured provider onto a built-in one: every field the
/// user set explicitly (i.e. differs from [`ModelProviderInfo::default`]) wins,
/// and anything they left unset keeps the built-in value.
fn overlay_configured_provider(base: &mut ModelProviderInfo, configured: ModelProviderInfo) {
    // Destructured so a new `ModelProviderInfo` field cannot be silently
    // dropped from the overlay.
    let ModelProviderInfo {
        name,
        base_url,
        env_key,
        env_key_instructions,
        experimental_bearer_token,
        auth,
        aws,
        wire_api,
        query_params,
        http_headers,
        env_http_headers,
        request_max_retries,
        stream_max_retries,
        stream_idle_timeout_ms,
        websocket_connect_timeout_ms,
        requires_openai_auth,
        requires_forall_auth,
        auth_header,
        supports_websockets,
    } = configured;
    let defaults = ModelProviderInfo::default();
    // Auth mechanisms are mutually exclusive in the resolver: a present
    // `env_key` short-circuits and yields unauthenticated when that variable is
    // unset, never consulting `experimental_bearer_token` or command-backed
    // `auth`. So whichever mechanism the config picks must displace the
    // built-in's, or the built-in would shadow it.
    let configures_env_key = env_key.is_some();
    let configures_alternative_auth = experimental_bearer_token.is_some() || auth.is_some();

    if name != defaults.name {
        base.name = name;
    }
    if base_url.is_some() {
        base.base_url = base_url;
    }
    if env_key.is_some() {
        base.env_key = env_key;
    }
    if env_key_instructions.is_some() {
        base.env_key_instructions = env_key_instructions;
    }
    if experimental_bearer_token.is_some() {
        base.experimental_bearer_token = experimental_bearer_token;
    }
    if auth.is_some() {
        base.auth = auth;
    }
    if aws.is_some() {
        base.aws = aws;
    }
    if wire_api != defaults.wire_api {
        base.wire_api = wire_api;
    }
    if query_params.is_some() {
        base.query_params = query_params;
    }
    if http_headers.is_some() {
        base.http_headers = http_headers;
    }
    if env_http_headers.is_some() {
        base.env_http_headers = env_http_headers;
    }
    if request_max_retries.is_some() {
        base.request_max_retries = request_max_retries;
    }
    if stream_max_retries.is_some() {
        base.stream_max_retries = stream_max_retries;
    }
    if stream_idle_timeout_ms.is_some() {
        base.stream_idle_timeout_ms = stream_idle_timeout_ms;
    }
    if websocket_connect_timeout_ms.is_some() {
        base.websocket_connect_timeout_ms = websocket_connect_timeout_ms;
    }
    if requires_openai_auth != defaults.requires_openai_auth {
        base.requires_openai_auth = requires_openai_auth;
    }
    if auth_header.is_some() {
        base.auth_header = auth_header;
    }
    if supports_websockets != defaults.supports_websockets {
        base.supports_websockets = supports_websockets;
    }
    if configures_alternative_auth && !configures_env_key {
        base.env_key = None;
    }
    if configures_env_key && !configures_alternative_auth {
        base.experimental_bearer_token = None;
        base.auth = None;
    }

    // `requires_forall_auth` is deliberately not overlayable: it grants the
    // user's Forall OAuth token to a provider, so only built-ins may set it.
    let _ = requires_forall_auth;
}

pub fn create_oss_provider(default_provider_port: u16, wire_api: WireApi) -> ModelProviderInfo {
    // These CODEX_OSS_ environment variables are experimental: we may
    // switch to reading values from config.toml instead.
    let default_forall_oss_base_url = format!(
        "http://localhost:{forall_oss_port}/v1",
        forall_oss_port = std::env::var("CODEX_OSS_PORT")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(default_provider_port)
    );

    let forall_oss_base_url = std::env::var("CODEX_OSS_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(default_forall_oss_base_url);
    create_oss_provider_with_base_url(&forall_oss_base_url, wire_api)
}

pub fn create_oss_provider_with_base_url(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    ModelProviderInfo {
        name: "gpt-oss".into(),
        base_url: Some(base_url.into()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    }
}

#[cfg(test)]
#[path = "model_provider_info_tests.rs"]
mod tests;
