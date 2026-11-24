use forall_app_server_protocol::AuthMode;
use forall_app_server_protocol::AuthMode as ApiAuthMode;
use forall_config::types::AuthCredentialsStoreMode;
use forall_protocol::account::PlanType as AccountPlanType;
use forall_protocol::auth::RefreshTokenFailedError;
use forall_protocol::auth::RefreshTokenFailedReason;
use forall_protocol::config_types::ForcedLoginMethod;
use forall_protocol::config_types::ModelProviderAuthInfo;
use once_cell::sync::Lazy;
use std::collections::BTreeMap;
use std::env;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tracing::instrument;

use super::external_bearer::BearerTokenRefresher;
pub use crate::auth::bedrock_api_key::BedrockApiKeyAuth;
pub use crate::auth::storage::AuthDotJson;
pub use crate::auth::storage::AuthKeyringBackendKind;
use crate::auth::storage::create_auth_storage;
use crate::outbound_proxy::AuthRouteConfig;
use crate::token_data::TokenData;

/// Authentication mechanism used by the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForallAuth {
    ApiKey(ApiKeyAuth),
    BedrockApiKey(BedrockApiKeyAuth),
    Oauth(OauthAuth),
}

/// Forall identity OAuth tokens (access/refresh/id token) issued by the Forall
/// auth service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthAuth {
    tokens: TokenData,
}

impl OauthAuth {
    pub fn new(tokens: TokenData) -> Self {
        Self { tokens }
    }

    pub fn tokens(&self) -> &TokenData {
        &self.tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAuth {
    /// Primary bearer key (OpenAI, falling back to OpenRouter). `None` when
    /// auth.json only stores keys for other providers in `provider_keys`.
    api_key: Option<String>,
    openai_api_key: Option<String>,
    openrouter_api_key: Option<String>,
    /// Additional BYOK provider keys keyed by provider `env_key`.
    provider_keys: BTreeMap<String, String>,
}

const REFRESH_TOKEN_UNKNOWN_MESSAGE: &str = "Authentication could not be refreshed.";

/// Environment override for the Forall auth service base URL.
pub const FORALL_AUTH_BASE_URL_ENV_VAR: &str = "FORALL_AUTH_BASE_URL";
/// Default Forall auth service base URL.
pub const DEFAULT_FORALL_AUTH_BASE_URL: &str = "https://auth.forall.astrio.app";

/// Base URL of the Forall auth service plus the derived OAuth endpoint URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthBaseUrls {
    base: String,
}

impl AuthBaseUrls {
    /// Resolution order: `FORALL_AUTH_BASE_URL` env var, then the config value,
    /// then the built-in default.
    pub fn resolve(config_value: Option<&str>) -> Self {
        let base = read_non_empty_env_var(FORALL_AUTH_BASE_URL_ENV_VAR)
            .or_else(|| {
                config_value
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_FORALL_AUTH_BASE_URL.to_string());
        Self {
            base: base.trim_end_matches('/').to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    pub fn authorize_url(&self) -> String {
        format!("{}/oauth/authorize", self.base)
    }

    pub fn token_url(&self) -> String {
        format!("{}/oauth/token", self.base)
    }

    pub fn revoke_url(&self) -> String {
        format!("{}/oauth/revoke", self.base)
    }
}

pub const REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REFRESH_TOKEN_URL_OVERRIDE";
pub const REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "CODEX_REVOKE_TOKEN_URL_OVERRIDE";
pub const CLIENT_ID_OVERRIDE_ENV_VAR: &str = "CODEX_APP_SERVER_LOGIN_CLIENT_ID";
pub const FORALL_OAUTH_CLIENT_ID_ENV_VAR: &str = "FORALL_OAUTH_CLIENT_ID";
pub const GOOGLE_CLIENT_ID_ENV_VAR: &str = "GOOGLE_CLIENT_ID";

#[derive(Debug, thiserror::Error)]
pub enum RefreshTokenError {
    #[error("{0}")]
    Permanent(#[from] RefreshTokenFailedError),
    #[error(transparent)]
    Transient(#[from] std::io::Error),
}

impl RefreshTokenError {
    pub fn failed_reason(&self) -> Option<RefreshTokenFailedReason> {
        match self {
            Self::Permanent(error) => Some(error.reason),
            Self::Transient(_) => None,
        }
    }
}

impl From<RefreshTokenError> for std::io::Error {
    fn from(err: RefreshTokenError) -> Self {
        match err {
            RefreshTokenError::Permanent(failed) => std::io::Error::other(failed),
            RefreshTokenError::Transient(inner) => inner,
        }
    }
}

impl ApiKeyAuth {
    fn with_keys(
        api_key: Option<String>,
        openai_api_key: Option<String>,
        openrouter_api_key: Option<String>,
        provider_keys: BTreeMap<String, String>,
    ) -> Self {
        Self {
            api_key,
            openai_api_key,
            openrouter_api_key,
            provider_keys,
        }
    }
}

impl ForallAuth {
    fn from_auth_dot_json(auth_dot_json: AuthDotJson) -> Option<Self> {
        if let Some(auth) = auth_dot_json.bedrock_api_key
            && (auth_dot_json.preferred_provider_keys.is_empty()
                || auth_dot_json.auth_mode == Some(ApiAuthMode::BedrockApiKey))
        {
            return Some(Self::BedrockApiKey(auth));
        }

        // Forall OAuth tokens take precedence over stored BYOK API keys.
        if let Some(tokens) = auth_dot_json.tokens {
            return Some(Self::Oauth(OauthAuth::new(tokens)));
        }

        let openai_api_key = normalize_key(auth_dot_json.openai_api_key);
        let openrouter_api_key = normalize_key(auth_dot_json.openrouter_api_key);
        let provider_keys: BTreeMap<String, String> = auth_dot_json
            .provider_keys
            .into_iter()
            .filter(|(_, key)| !key.trim().is_empty())
            .collect();
        let api_key = openai_api_key
            .clone()
            .or_else(|| openrouter_api_key.clone());

        // A map-only auth.json (keys for providers other than OpenAI/OpenRouter)
        // still constitutes API-key auth; only a fully empty file yields None.
        // The primary bearer stays derived from the legacy fields exclusively so
        // another provider's key is never sent to OpenAI-authenticated endpoints.
        if api_key.is_none() && provider_keys.is_empty() {
            return None;
        }

        Some(Self::ApiKey(ApiKeyAuth::with_keys(
            api_key,
            openai_api_key,
            openrouter_api_key,
            provider_keys,
        )))
    }

    pub async fn from_auth_storage(
        forall_home: &Path,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<&AuthRouteConfig>,
    ) -> std::io::Result<Option<Self>> {
        load_auth(
            forall_home,
            /*enable_forall_api_key_env*/ false,
            auth_credentials_store_mode,
            keyring_backend_kind,
            auth_route_config,
        )
        .await
    }

    pub fn auth_mode(&self) -> AuthMode {
        match self {
            Self::ApiKey(_) => AuthMode::ApiKey,
            Self::BedrockApiKey(_) => AuthMode::BedrockApiKey,
            Self::Oauth(_) => AuthMode::Oauth,
        }
    }

    pub fn api_auth_mode(&self) -> ApiAuthMode {
        self.auth_mode()
    }

    pub fn is_api_key_auth(&self) -> bool {
        matches!(self, Self::ApiKey(_))
    }

    pub fn is_chatgpt_auth(&self) -> bool {
        false
    }

    pub fn is_forall_oauth(&self) -> bool {
        matches!(self, Self::Oauth(_))
    }

    /// Returns true when this auth can serve as a Forall identity (OAuth) login.
    pub fn identity_auth_available(&self) -> bool {
        matches!(self, Self::Oauth(_))
    }

    pub fn uses_forall_backend(&self) -> bool {
        matches!(self, Self::Oauth(_))
    }

    pub fn is_forall_local_identity(&self) -> bool {
        false
    }

    /// Returns a provider API key persisted in auth.json for the given env var name.
    ///
    /// Each provider resolves strictly to its own stored key; an unknown or
    /// unset `env_key` returns `None` rather than another provider's key.
    pub fn stored_provider_env_api_key(&self, env_key: &str) -> Option<String> {
        match self {
            Self::ApiKey(auth) => match env_key {
                OPENAI_API_KEY_ENV_VAR => auth.openai_api_key.clone(),
                OPENROUTER_API_KEY_ENV_VAR => auth.openrouter_api_key.clone(),
                _ => auth.provider_keys.get(env_key).cloned(),
            },
            Self::BedrockApiKey(_) | Self::Oauth(_) => None,
        }
    }

    pub fn is_external_chatgpt_tokens(&self) -> bool {
        false
    }

    /// Returns `None` if `auth_mode() != AuthMode::ApiKey` or when only
    /// non-primary provider keys are stored.
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(auth) => auth.api_key.as_deref(),
            Self::BedrockApiKey(_) | Self::Oauth(_) => None,
        }
    }

    /// Returns stored OAuth tokens; errors for API-key backed auth.
    pub fn get_token_data(&self) -> Result<TokenData, std::io::Error> {
        match self {
            Self::Oauth(auth) => Ok(auth.tokens.clone()),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => {
                Err(std::io::Error::other("Token data is not available."))
            }
        }
    }

    /// Returns the token string used for bearer authentication.
    pub fn get_token(&self) -> Result<String, std::io::Error> {
        match self {
            Self::ApiKey(auth) => auth.api_key.clone().ok_or_else(|| {
                std::io::Error::other(
                    "No primary API key is stored; only provider-specific keys are available.",
                )
            }),
            Self::BedrockApiKey(_) => Err(std::io::Error::other(
                "Bedrock API key auth does not expose a Forall bearer token",
            )),
            Self::Oauth(auth) => Ok(auth.tokens.access_token.clone()),
        }
    }

    /// Returns the account id associated with OAuth auth, `None` in BYOK mode.
    pub fn get_account_id(&self) -> Option<String> {
        match self {
            Self::Oauth(auth) => auth
                .tokens
                .account_id
                .clone()
                .or_else(|| auth.tokens.id_token.chatgpt_account_id.clone()),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => None,
        }
    }

    /// Returns false in BYOK mode.
    pub fn is_fedramp_account(&self) -> bool {
        match self {
            Self::Oauth(auth) => auth.tokens.id_token.is_fedramp_account(),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => false,
        }
    }

    /// Returns the signed-in email for OAuth auth, `None` in BYOK mode.
    pub fn get_account_email(&self) -> Option<String> {
        match self {
            Self::Oauth(auth) => auth.tokens.id_token.email.clone(),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => None,
        }
    }

    /// Returns the signed-in display name for OAuth auth, `None` in BYOK mode.
    pub fn get_account_name(&self) -> Option<String> {
        match self {
            Self::Oauth(auth) => auth.tokens.id_token.name.clone(),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => None,
        }
    }

    /// Returns the user id claim for OAuth auth, `None` in BYOK mode.
    pub fn get_chatgpt_user_id(&self) -> Option<String> {
        match self {
            Self::Oauth(auth) => auth.tokens.id_token.chatgpt_user_id.clone(),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => None,
        }
    }

    /// Account-facing plan classification derived from the current auth.
    pub fn account_plan_type(&self) -> Option<AccountPlanType> {
        match self {
            Self::Oauth(auth) => auth
                .tokens
                .id_token
                .get_chatgpt_plan_type_raw()
                .map(|raw| forall_protocol::auth::PlanType::from_raw_value(&raw).into()),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => None,
        }
    }

    pub fn is_workspace_account(&self) -> bool {
        match self {
            Self::Oauth(auth) => auth.tokens.id_token.is_workspace_account(),
            Self::ApiKey(_) | Self::BedrockApiKey(_) => false,
        }
    }

    /// Consider this private to integration tests.
    pub fn create_dummy_api_key_auth_for_testing() -> Self {
        Self::from_api_key("dummy-api-key")
    }

    pub fn from_api_key(api_key: &str) -> Self {
        Self::ApiKey(ApiKeyAuth::with_keys(
            Some(api_key.to_string()),
            Some(api_key.to_string()),
            None,
            BTreeMap::new(),
        ))
    }
}

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const OPENROUTER_API_KEY_ENV_VAR: &str = "OPENROUTER_API_KEY";
pub const AZURE_OPENAI_API_KEY_ENV_VAR: &str = "AZURE_OPENAI_API_KEY";
pub const ANTHROPIC_API_KEY_ENV_VAR: &str = "ANTHROPIC_API_KEY";
pub const GEMINI_API_KEY_ENV_VAR: &str = "GEMINI_API_KEY";
/// Forall API key env var (dashboard / cloud / verification).
pub const FORALL_API_KEY_ENV_VAR: &str = "FORALL_API_KEY";

pub fn read_openai_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(OPENAI_API_KEY_ENV_VAR)
}

pub fn read_openrouter_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(OPENROUTER_API_KEY_ENV_VAR)
}

pub fn read_azure_openai_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(AZURE_OPENAI_API_KEY_ENV_VAR)
}

pub fn read_anthropic_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(ANTHROPIC_API_KEY_ENV_VAR)
}

pub fn read_gemini_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(GEMINI_API_KEY_ENV_VAR)
}

pub fn read_forall_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(FORALL_API_KEY_ENV_VAR)
}

fn read_non_empty_env_var(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_key(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Delete the auth.json file inside `forall_home` if it exists. Returns `Ok(true)`
/// if a file was removed, `Ok(false)` if no auth file was present.
pub fn logout(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let _mutation_guard = lock_auth_mutation();
    storage.delete()
}

pub async fn logout_with_revoke(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    _auth_route_config: Option<&AuthRouteConfig>,
) -> std::io::Result<bool> {
    logout_all_stores(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Best-effort revocation of the stored Forall OAuth refresh token.
///
/// Posts the stored refresh token to the auth service's revoke endpoint and
/// ignores every failure; callers should proceed with local logout regardless
/// of the outcome. A no-op when no OAuth tokens are stored.
pub async fn revoke_stored_oauth_tokens_best_effort(
    forall_home: &Path,
    base_urls: &AuthBaseUrls,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) {
    let Some(tokens) = load_auth_dot_json(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
    .ok()
    .flatten()
    .and_then(|auth| auth.tokens) else {
        return;
    };

    let request = crate::auth::default_client::credential_request(
        reqwest::Method::POST,
        &base_urls.revoke_url(),
    );
    let Ok(request) = request else {
        tracing::warn!("could not prepare the auth revocation request");
        return;
    };
    match request
        .form(&[("token", tokens.refresh_token.as_str())])
        .send()
        .await
    {
        Ok(response) if !response.status().is_success() => {
            tracing::warn!(
                "revoke endpoint returned HTTP {} during logout",
                response.status()
            );
        }
        Ok(_) => {}
        Err(err) => {
            tracing::warn!("failed to revoke refresh token during logout: {err}");
        }
    }
}

/// Writes an `auth.json` that contains only the API key.
pub fn login_with_api_key(
    forall_home: &Path,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(ApiAuthMode::ApiKey),
        openai_api_key: Some(api_key.to_string()),
        openrouter_api_key: None,
        provider_keys: BTreeMap::new(),
        preferred_provider_keys: Default::default(),
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };
    save_auth(
        forall_home,
        &auth_dot_json,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Persists the GitHub connection, preserving every other stored credential.
pub fn save_github_connection(
    forall_home: &Path,
    connection: &crate::github_connect::GithubConnection,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let _mutation_guard = lock_auth_mutation();
    let mut auth_dot_json = storage.load()?.unwrap_or_default();
    auth_dot_json.github_connection = Some(connection.clone());
    storage.save(&auth_dot_json)
}

/// Reads the stored GitHub connection, if any.
pub fn load_github_connection(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<Option<crate::github_connect::GithubConnection>> {
    Ok(load_auth_dot_json(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )?
    .and_then(|auth| auth.github_connection))
}

/// Removes the stored GitHub connection, preserving every other credential.
/// Returns whether a connection was present.
pub fn clear_github_connection(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let _mutation_guard = lock_auth_mutation();
    let Some(mut auth_dot_json) = storage.load()? else {
        return Ok(false);
    };
    let present = auth_dot_json.github_connection.is_some();
    if present {
        auth_dot_json.github_connection = None;
        storage.save(&auth_dot_json)?;
    }
    Ok(present)
}

/// Serializes every load-modify-save of the credential store within this
/// process. Two concurrent whole-record writes (say, a token refresh
/// persisting a rotated — single-use — refresh token while a GitHub connect
/// flow saves its token) would otherwise race read-vs-write and one would
/// resurrect the state the other just replaced.
static AUTH_MUTATION_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn lock_auth_mutation() -> std::sync::MutexGuard<'static, ()> {
    // The guarded value is (), so a panic while holding it cannot leave
    // broken state; recover rather than poison every later save.
    AUTH_MUTATION_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Persists Forall OAuth tokens, preserving any stored BYOK provider keys.
///
/// The refresh token is single-use on the server, so callers must invoke this
/// immediately whenever a token response carries a rotated refresh token.
pub fn save_oauth_tokens(
    forall_home: &Path,
    tokens: TokenData,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let _mutation_guard = lock_auth_mutation();
    let mut auth_dot_json = storage.load()?.unwrap_or_default();
    auth_dot_json.auth_mode = Some(ApiAuthMode::Oauth);
    auth_dot_json.tokens = Some(tokens);
    auth_dot_json.last_refresh = Some(chrono::Utc::now());
    // Bedrock auth takes precedence when loading, so it must be cleared for the
    // new OAuth login to become active. BYOK provider keys are preserved.
    auth_dot_json.bedrock_api_key = None;
    storage.save(&auth_dot_json)
}

/// Stores an OpenAI API key for BYOK chat.
pub fn set_openai_api_key(
    forall_home: &Path,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    set_provider_api_key(
        forall_home,
        OPENAI_API_KEY_ENV_VAR,
        api_key,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Stores an OpenRouter API key for BYOK chat.
pub fn set_openrouter_api_key(
    forall_home: &Path,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    set_provider_api_key(
        forall_home,
        OPENROUTER_API_KEY_ENV_VAR,
        api_key,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Maps a BYOK model provider id to the env var its API key is stored under.
///
/// Returns `None` for providers without a known key slot so callers fail
/// loudly instead of silently storing the key under a different provider.
fn byok_env_key_for_provider(model_provider_id: &str) -> Option<&'static str> {
    match model_provider_id {
        "openai" => Some(OPENAI_API_KEY_ENV_VAR),
        "openrouter" => Some(OPENROUTER_API_KEY_ENV_VAR),
        "azure" => Some(AZURE_OPENAI_API_KEY_ENV_VAR),
        "anthropic" => Some(ANTHROPIC_API_KEY_ENV_VAR),
        "gemini" => Some(GEMINI_API_KEY_ENV_VAR),
        _ => None,
    }
}

/// Save an explicitly selected provider key alongside the user's other credentials.
pub fn save_preferred_provider_key(
    forall_home: &Path,
    provider_id: &str,
    api_key: &str,
    store_mode: AuthCredentialsStoreMode,
    keyring_backend: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let env_key = byok_env_key_for_provider(provider_id)
        .ok_or_else(|| std::io::Error::other("unknown API-key provider"))?;
    let storage = create_auth_storage(forall_home.to_path_buf(), store_mode, keyring_backend);
    let _guard = lock_auth_mutation();
    let mut auth = storage.load()?.unwrap_or_default();
    if auth.tokens.is_none() {
        auth.auth_mode = Some(ApiAuthMode::ApiKey);
    }
    auth.set_provider_key(env_key, api_key.to_owned());
    auth.preferred_provider_keys.insert(env_key.to_owned());
    storage.save(&auth)
}

/// Stores a BYOK API key for the requested model provider.
///
/// Errors on provider ids without a known key slot; a key is never written
/// to a different provider's slot.
pub fn set_byok_api_key(
    forall_home: &Path,
    model_provider_id: &str,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let env_key = byok_env_key_for_provider(model_provider_id).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unsupported BYOK model provider: {model_provider_id}"),
        )
    })?;
    set_provider_api_key(
        forall_home,
        env_key,
        api_key,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

fn set_provider_api_key(
    forall_home: &Path,
    env_key: &str,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let _mutation_guard = lock_auth_mutation();
    let mut auth_dot_json = storage.load()?.unwrap_or_default();
    auth_dot_json.auth_mode = Some(ApiAuthMode::ApiKey);
    auth_dot_json.tokens = None;
    auth_dot_json.last_refresh = None;
    auth_dot_json.bedrock_api_key = None;
    auth_dot_json.set_provider_key(env_key, api_key.to_string());
    storage.save(&auth_dot_json)
}

/// Returns true when persisted auth includes a non-empty OpenAI API key.
pub fn stored_openai_api_key_is_set(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    stored_provider_api_key_is_set(
        forall_home,
        OPENAI_API_KEY_ENV_VAR,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Returns true when persisted auth includes a non-empty OpenRouter API key.
pub fn stored_openrouter_api_key_is_set(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    stored_provider_api_key_is_set(
        forall_home,
        OPENROUTER_API_KEY_ENV_VAR,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

fn stored_provider_api_key_is_set(
    forall_home: &Path,
    env_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    load_auth_dot_json(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
    .ok()
    .flatten()
    .is_some_and(|auth| auth.provider_key(env_key).is_some())
}

/// Load BYOK API keys from `~/.forall/.env` and the current working directory `.env`.
///
/// Existing non-empty process environment variables are not overwritten.
pub fn load_byok_env_files(forall_home: &Path) {
    load_env_file_if_present(&forall_home.join(".env"));
    if let Ok(cwd) = std::env::current_dir() {
        load_env_file_if_present(&cwd.join(".env"));
    }
}

/// Reads a provider API key from the process env or `.env` files.
pub fn read_provider_key_from_env_files(
    forall_home: &Path,
    workspace_cwd: Option<&Path>,
    env_key: &str,
) -> Option<String> {
    if let Some(value) = read_non_empty_env_var(env_key) {
        return Some(value);
    }
    let mut candidates = vec![forall_home.join(".env")];
    if let Some(cwd) = workspace_cwd {
        candidates.push(cwd.join(".env"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".env"));
    }
    for path in candidates {
        for (key, value) in forall_utils_home_dir::dotenv_entries(&path) {
            if key == env_key && !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

fn load_env_file_if_present(path: &Path) {
    for (key, value) in forall_utils_home_dir::dotenv_entries(path) {
        if key.to_ascii_uppercase().starts_with("CODEX_") {
            continue;
        }
        if std::env::var_os(&key).is_some_and(|existing| !existing.is_empty()) {
            continue;
        }
        // Safe: onboarding calls this from the UI thread before spawning save tasks.
        unsafe {
            std::env::set_var(&key, &value);
        }
    }
}

/// When true, Forall identity auth is not required or surfaced by the app.
///
/// This auth-unaware fallback stays `true` so pure BYOK flows behave exactly
/// as before. Call sites that have a `ForallAuth` in scope should prefer
/// [`ForallAuth::identity_auth_available`], which reports identity auth as
/// enabled when the active auth is Forall OAuth.
pub fn forall_identity_auth_usage_disabled() -> bool {
    true
}

/// Returns true when the provider API key is available from env or auth.json.
pub fn provider_env_api_key_is_configured(
    env_key: &str,
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    read_non_empty_env_var(env_key).is_some()
        || stored_provider_api_key_is_set(
            forall_home,
            env_key,
            auth_credentials_store_mode,
            keyring_backend_kind,
        )
}

/// Returns true when stored auth tokens indicate a Forall local identity login.
pub fn forall_local_identity_on_disk(
    _forall_home: &Path,
    _auth_credentials_store_mode: AuthCredentialsStoreMode,
    _keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    false
}

/// Returns true when the user completed BYOK provider onboarding.
pub fn user_config_has_byok_provider_choice(forall_home: &Path) -> bool {
    config_toml_has_key(forall_home, "byok_provider")
}

fn config_toml_has_key(forall_home: &Path, key: &str) -> bool {
    let path = forall_home.join("config.toml");
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return false,
    };
    content.lines().any(|line| {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        trimmed.starts_with(key) && trimmed.contains('=')
    })
}

/// Returns true when stored auth carries Forall OAuth tokens.
pub fn forall_oauth_tokens_on_disk(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    load_auth_dot_json(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
    .ok()
    .flatten()
    .is_some_and(|auth| auth.tokens.is_some())
}

/// Forall sign-in uses BYOK; show provider selection until the user picks one
/// or signs in with Forall OAuth.
pub fn forall_needs_byok_provider_selection(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> bool {
    if user_config_has_byok_provider_choice(forall_home) {
        return false;
    }
    if forall_oauth_tokens_on_disk(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    ) {
        return false;
    }
    if forall_identity_auth_usage_disabled() {
        return true;
    }
    forall_local_identity_on_disk(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Reads a provider API key from auth.json on disk.
///
/// Each provider resolves strictly to its own stored key; an unknown or
/// unset `env_key` returns `None` rather than another provider's key.
pub fn read_stored_provider_api_key_from_disk(
    forall_home: &Path,
    env_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> Option<String> {
    load_auth_dot_json(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
    .ok()
    .flatten()
    .and_then(|auth| auth.provider_key(env_key).map(str::to_string))
}

/// Persist the provided auth payload using the specified backend.
pub fn save_auth(
    forall_home: &Path,
    auth: &AuthDotJson,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    // Whole-record overwrites (API-key and Bedrock logins land here) must
    // serialize with the locked load-modify-save composites above, or an
    // interleaved save silently discards one side's credential state.
    let _mutation_guard = lock_auth_mutation();
    storage.save(auth)
}

/// Load the raw stored auth payload without applying environment overrides.
pub fn load_auth_dot_json(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<Option<AuthDotJson>> {
    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    storage.load()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub forall_home: PathBuf,
    pub auth_credentials_store_mode: AuthCredentialsStoreMode,
    pub keyring_backend_kind: AuthKeyringBackendKind,
    pub forced_login_method: Option<ForcedLoginMethod>,
    pub auth_route_config: Option<AuthRouteConfig>,
}

/// Enforces configured login restrictions using auth-owned HTTP settings.
pub async fn enforce_login_restrictions(config: &AuthConfig) -> std::io::Result<()> {
    let Some(auth) = load_auth(
        &config.forall_home,
        /*enable_forall_api_key_env*/ true,
        config.auth_credentials_store_mode,
        config.keyring_backend_kind,
        config.auth_route_config.as_ref(),
    )
    .await?
    else {
        return Ok(());
    };

    if let Some(required_method) = config.forced_login_method
        && required_method == ForcedLoginMethod::Api
        && !matches!(auth.auth_mode(), AuthMode::ApiKey | AuthMode::BedrockApiKey)
    {
        return logout_with_message(
            &config.forall_home,
            "API key login is required. Logging out.".to_string(),
            config.auth_credentials_store_mode,
            config.keyring_backend_kind,
        );
    }

    Ok(())
}

fn logout_with_message(
    forall_home: &Path,
    message: String,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let removal_result = logout_all_stores(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let error_message = match removal_result {
        Ok(_) => message,
        Err(err) => format!("{message}. Failed to remove auth.json: {err}"),
    };
    Err(std::io::Error::other(error_message))
}

fn logout_all_stores(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    if auth_credentials_store_mode == AuthCredentialsStoreMode::Ephemeral {
        return logout(
            forall_home,
            AuthCredentialsStoreMode::Ephemeral,
            AuthKeyringBackendKind::default(),
        );
    }
    let removed_ephemeral = logout(
        forall_home,
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::default(),
    )?;
    let removed_managed = logout(
        forall_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )?;
    Ok(removed_ephemeral || removed_managed)
}

#[allow(clippy::too_many_arguments)]
async fn load_auth(
    forall_home: &Path,
    enable_forall_api_key_env: bool,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    _auth_route_config: Option<&AuthRouteConfig>,
) -> std::io::Result<Option<ForallAuth>> {
    // API key via env var takes precedence over any other auth method.
    if enable_forall_api_key_env && let Some(api_key) = read_forall_api_key_from_env() {
        return Ok(Some(ForallAuth::from_api_key(api_key.as_str())));
    }

    let storage = create_auth_storage(
        forall_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let Some(auth_dot_json) = storage.load()? else {
        return Ok(None);
    };

    let parsed = ForallAuth::from_auth_dot_json(auth_dot_json);
    if parsed.is_none() {
        tracing::debug!("ignoring unsupported legacy auth payload");
    }
    Ok(parsed)
}

/// Shared constant for token refresh client id.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub fn oauth_client_id() -> String {
    std::env::var(FORALL_OAUTH_CLIENT_ID_ENV_VAR)
        .or_else(|_| std::env::var(GOOGLE_CLIENT_ID_ENV_VAR))
        .or_else(|_| std::env::var(CLIENT_ID_OVERRIDE_ENV_VAR))
        .ok()
        .filter(|client_id| !client_id.trim().is_empty())
        .unwrap_or_else(|| CLIENT_ID.to_string())
}

/// Internal cached auth state.
#[derive(Clone)]
struct CachedAuth {
    auth: Option<ForallAuth>,
    permanent_refresh_failure: Option<AuthScopedRefreshFailure>,
}

#[derive(Clone)]
struct AuthScopedRefreshFailure {
    auth: ForallAuth,
    error: RefreshTokenFailedError,
}

impl Debug for CachedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAuth")
            .field(
                "auth_mode",
                &self.auth.as_ref().map(ForallAuth::api_auth_mode),
            )
            .field(
                "permanent_refresh_failure",
                &self
                    .permanent_refresh_failure
                    .as_ref()
                    .map(|failure| failure.error.reason),
            )
            .finish()
    }
}

enum UnauthorizedRecoveryStep {
    ExternalRefresh,
    ManagedOauthRefresh,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnauthorizedRecoveryMode {
    Managed,
    External,
}

pub struct UnauthorizedRecovery {
    manager: Arc<AuthManager>,
    step: UnauthorizedRecoveryStep,
    mode: UnauthorizedRecoveryMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnauthorizedRecoveryStepResult {
    auth_state_changed: Option<bool>,
}

impl UnauthorizedRecoveryStepResult {
    pub fn auth_state_changed(&self) -> Option<bool> {
        self.auth_state_changed
    }
}

impl UnauthorizedRecovery {
    fn new(manager: Arc<AuthManager>) -> Self {
        let mode = if manager.has_external_api_key_auth() {
            UnauthorizedRecoveryMode::External
        } else {
            UnauthorizedRecoveryMode::Managed
        };
        let step = match mode {
            UnauthorizedRecoveryMode::External => UnauthorizedRecoveryStep::ExternalRefresh,
            // Managed Forall OAuth tokens can be refreshed once on a 401; other
            // managed auth (API keys) has nothing to refresh.
            UnauthorizedRecoveryMode::Managed => {
                if matches!(manager.auth_cached(), Some(ForallAuth::Oauth(_))) {
                    UnauthorizedRecoveryStep::ManagedOauthRefresh
                } else {
                    UnauthorizedRecoveryStep::Done
                }
            }
        };
        Self {
            manager,
            step,
            mode,
        }
    }

    pub fn has_next(&self) -> bool {
        !matches!(self.step, UnauthorizedRecoveryStep::Done)
    }

    pub fn unavailable_reason(&self) -> &'static str {
        if matches!(self.step, UnauthorizedRecoveryStep::Done) {
            "recovery_exhausted"
        } else {
            "ready"
        }
    }

    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            UnauthorizedRecoveryMode::Managed => "managed",
            UnauthorizedRecoveryMode::External => "external",
        }
    }

    pub fn step_name(&self) -> &'static str {
        match self.step {
            UnauthorizedRecoveryStep::ExternalRefresh => "external_refresh",
            UnauthorizedRecoveryStep::ManagedOauthRefresh => "managed_oauth_refresh",
            UnauthorizedRecoveryStep::Done => "done",
        }
    }

    pub async fn next(&mut self) -> Result<UnauthorizedRecoveryStepResult, RefreshTokenError> {
        if !self.has_next() {
            return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                "No more recovery steps available.",
            )));
        }

        match self.step {
            UnauthorizedRecoveryStep::ExternalRefresh => {
                self.manager.refresh_external_api_key().await?;
                self.step = UnauthorizedRecoveryStep::Done;
                Ok(UnauthorizedRecoveryStepResult {
                    auth_state_changed: Some(true),
                })
            }
            UnauthorizedRecoveryStep::ManagedOauthRefresh => {
                // `refresh_token` persists the rotated refresh token before
                // updating the cached auth, so the retry observes fresh tokens.
                self.manager.refresh_token().await?;
                self.step = UnauthorizedRecoveryStep::Done;
                Ok(UnauthorizedRecoveryStepResult {
                    auth_state_changed: Some(true),
                })
            }
            UnauthorizedRecoveryStep::Done => Ok(UnauthorizedRecoveryStepResult {
                auth_state_changed: None,
            }),
        }
    }
}

/// Central manager providing a single source of truth for auth.json derived
/// authentication data.
pub struct AuthManager {
    forall_home: PathBuf,
    inner: RwLock<CachedAuth>,
    auth_change_tx: watch::Sender<u64>,
    enable_forall_api_key_env: bool,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    refresh_lock: Semaphore,
    external_api_key_auth: RwLock<Option<Arc<BearerTokenRefresher>>>,
    auth_route_config: Option<AuthRouteConfig>,
    auth_base_urls: AuthBaseUrls,
}

pub trait AuthManagerConfig {
    fn forall_home(&self) -> PathBuf;
    fn cli_auth_credentials_store_mode(&self) -> AuthCredentialsStoreMode;
    fn auth_keyring_backend_kind(&self) -> AuthKeyringBackendKind;
    fn auth_route_config(&self) -> Option<AuthRouteConfig>;
    /// Optional config override for the Forall auth service base URL.
    fn forall_auth_base_url(&self) -> Option<String> {
        None
    }
}

impl Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("forall_home", &self.forall_home)
            .field("inner", &self.inner)
            .field("enable_forall_api_key_env", &self.enable_forall_api_key_env)
            .field(
                "auth_credentials_store_mode",
                &self.auth_credentials_store_mode,
            )
            .field("keyring_backend_kind", &self.keyring_backend_kind)
            .field("auth_route_config", &self.auth_route_config)
            .field("has_external_auth", &self.has_external_auth())
            .finish_non_exhaustive()
    }
}

impl AuthManager {
    pub async fn new(
        forall_home: PathBuf,
        enable_forall_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<AuthRouteConfig>,
        auth_base_urls: Option<AuthBaseUrls>,
    ) -> Self {
        let managed_auth = load_auth(
            &forall_home,
            enable_forall_api_key_env,
            auth_credentials_store_mode,
            keyring_backend_kind,
            auth_route_config.as_ref(),
        )
        .await
        .ok()
        .flatten();
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Self {
            forall_home,
            inner: RwLock::new(CachedAuth {
                auth: managed_auth,
                permanent_refresh_failure: None,
            }),
            auth_change_tx,
            enable_forall_api_key_env,
            auth_credentials_store_mode,
            keyring_backend_kind,
            refresh_lock: Semaphore::new(1),
            external_api_key_auth: RwLock::new(None),
            auth_route_config,
            auth_base_urls: auth_base_urls.unwrap_or_else(|| AuthBaseUrls::resolve(None)),
        }
    }

    pub fn from_auth_for_testing(auth: ForallAuth) -> Arc<Self> {
        let cached = CachedAuth {
            auth: Some(auth),
            permanent_refresh_failure: None,
        };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            forall_home: PathBuf::from("non-existent"),
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_forall_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            refresh_lock: Semaphore::new(1),
            external_api_key_auth: RwLock::new(None),
            auth_route_config: None,
            auth_base_urls: AuthBaseUrls::resolve(None),
        })
    }

    pub fn from_auth_for_testing_with_home(auth: ForallAuth, forall_home: PathBuf) -> Arc<Self> {
        let cached = CachedAuth {
            auth: Some(auth),
            permanent_refresh_failure: None,
        };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            forall_home,
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_forall_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            refresh_lock: Semaphore::new(1),
            external_api_key_auth: RwLock::new(None),
            auth_route_config: None,
            auth_base_urls: AuthBaseUrls::resolve(None),
        })
    }

    pub fn external_bearer_only(config: ModelProviderAuthInfo) -> Arc<Self> {
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            forall_home: PathBuf::from("non-existent"),
            inner: RwLock::new(CachedAuth {
                auth: None,
                permanent_refresh_failure: None,
            }),
            auth_change_tx,
            enable_forall_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            refresh_lock: Semaphore::new(1),
            external_api_key_auth: RwLock::new(Some(Arc::new(BearerTokenRefresher::new(config)))),
            auth_route_config: None,
            auth_base_urls: AuthBaseUrls::resolve(None),
        })
    }

    pub fn auth_cached(&self) -> Option<ForallAuth> {
        self.inner.read().ok().and_then(|c| c.auth.clone())
    }

    pub fn auth_change_receiver(&self) -> watch::Receiver<u64> {
        self.auth_change_tx.subscribe()
    }

    pub fn refresh_failure_for_auth(&self, auth: &ForallAuth) -> Option<RefreshTokenFailedError> {
        self.inner.read().ok().and_then(|cached| {
            cached
                .permanent_refresh_failure
                .as_ref()
                .filter(|failure| Self::auths_equal_for_refresh(Some(auth), Some(&failure.auth)))
                .map(|failure| failure.error.clone())
        })
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn auth(&self) -> Option<ForallAuth> {
        if let Some(auth) = self.resolve_external_api_key_auth().await {
            return Some(auth);
        }
        self.auth_cached()
    }

    fn provider_key_record(&self) -> Option<AuthDotJson> {
        load_auth_dot_json(
            &self.forall_home,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )
        .ok()
        .flatten()
    }

    pub fn preferred_provider_api_key(&self, env_key: &str) -> Option<String> {
        self.provider_key_record()?
            .preferred_provider_key(env_key)
            .map(str::to_owned)
    }

    /// Resolve an explicitly selected credential before the usual environment-first lookup.
    pub fn provider_api_key(&self, env_key: &str, auth: Option<&ForallAuth>) -> Option<String> {
        let record = self.provider_key_record();
        record
            .as_ref()
            .and_then(|r| r.preferred_provider_key(env_key))
            .map(str::to_owned)
            .or_else(|| read_non_empty_env_var(env_key))
            .or_else(|| auth.and_then(|auth| auth.stored_provider_env_api_key(env_key)))
            .or_else(|| record.as_ref()?.provider_key(env_key).map(str::to_owned))
    }

    pub fn stored_env_api_key_from_disk(&self, env_key: &str) -> Option<String> {
        read_stored_provider_api_key_from_disk(
            &self.forall_home,
            env_key,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )
    }

    pub async fn reload(&self) -> bool {
        tracing::info!("Reloading auth");
        let new_auth = self.load_auth_from_storage().await;
        self.set_cached_auth(new_auth)
    }

    fn auths_equal_for_refresh(a: Option<&ForallAuth>, b: Option<&ForallAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => match (a.api_auth_mode(), b.api_auth_mode()) {
                (ApiAuthMode::ApiKey, ApiAuthMode::ApiKey) => a.api_key() == b.api_key(),
                (ApiAuthMode::BedrockApiKey, ApiAuthMode::BedrockApiKey)
                | (ApiAuthMode::Oauth, ApiAuthMode::Oauth) => a == b,
                _ => false,
            },
            _ => false,
        }
    }

    fn auths_equal(a: Option<&ForallAuth>, b: Option<&ForallAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    async fn load_auth_from_storage(&self) -> Option<ForallAuth> {
        load_auth(
            &self.forall_home,
            self.enable_forall_api_key_env,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
            self.auth_route_config.as_ref(),
        )
        .await
        .ok()
        .flatten()
    }

    fn set_cached_auth(&self, new_auth: Option<ForallAuth>) -> bool {
        if let Ok(mut guard) = self.inner.write() {
            let previous = guard.auth.as_ref();
            let changed = !AuthManager::auths_equal(previous, new_auth.as_ref());
            let auth_changed_for_refresh =
                !Self::auths_equal_for_refresh(previous, new_auth.as_ref());
            if auth_changed_for_refresh {
                guard.permanent_refresh_failure = None;
            }
            guard.auth = new_auth;
            if auth_changed_for_refresh {
                self.auth_change_tx.send_modify(|revision| *revision += 1);
            }
            changed
        } else {
            false
        }
    }

    pub fn clear_external_auth(&self) {
        if let Ok(mut guard) = self.external_api_key_auth.write() {
            *guard = None;
        }
    }

    pub fn has_external_auth(&self) -> bool {
        self.has_external_api_key_auth()
    }

    pub fn is_external_chatgpt_auth_active(&self) -> bool {
        false
    }

    pub fn forall_api_key_env_enabled(&self) -> bool {
        self.enable_forall_api_key_env
    }

    pub async fn shared(
        forall_home: PathBuf,
        enable_forall_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<AuthRouteConfig>,
        auth_base_urls: Option<AuthBaseUrls>,
    ) -> Arc<Self> {
        Arc::new(
            Self::new(
                forall_home,
                enable_forall_api_key_env,
                auth_credentials_store_mode,
                keyring_backend_kind,
                auth_route_config,
                auth_base_urls,
            )
            .await,
        )
    }

    pub async fn shared_from_config(
        config: &impl AuthManagerConfig,
        enable_forall_api_key_env: bool,
    ) -> Arc<Self> {
        Self::shared(
            config.forall_home(),
            enable_forall_api_key_env,
            config.cli_auth_credentials_store_mode(),
            config.auth_keyring_backend_kind(),
            config.auth_route_config(),
            Some(AuthBaseUrls::resolve(
                config.forall_auth_base_url().as_deref(),
            )),
        )
        .await
    }

    pub fn unauthorized_recovery(self: &Arc<Self>) -> UnauthorizedRecovery {
        UnauthorizedRecovery::new(Arc::clone(self))
    }

    fn has_external_api_key_auth(&self) -> bool {
        self.external_api_key_auth
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .is_some()
    }

    async fn resolve_external_api_key_auth(&self) -> Option<ForallAuth> {
        let external_auth = self
            .external_api_key_auth
            .read()
            .ok()
            .and_then(|guard| guard.clone())?;

        match external_auth.resolve().await {
            Ok(Some(token)) => Some(ForallAuth::from_api_key(&token)),
            Ok(None) => None,
            Err(err) => {
                tracing::error!("Failed to resolve external API key auth: {err}");
                None
            }
        }
    }

    async fn refresh_external_api_key(&self) -> Result<(), RefreshTokenError> {
        let external_auth = self
            .external_api_key_auth
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .ok_or_else(|| {
                RefreshTokenError::Transient(std::io::Error::other(
                    "external auth is not configured",
                ))
            })?;
        let token = external_auth
            .refresh()
            .await
            .map_err(RefreshTokenError::Transient)?;
        let changed = self.set_cached_auth(Some(ForallAuth::from_api_key(&token)));
        if !changed {
            self.auth_change_tx.send_modify(|revision| *revision += 1);
        }
        Ok(())
    }

    pub async fn refresh_token(&self) -> Result<(), RefreshTokenError> {
        let _refresh_guard = self.refresh_lock.acquire().await.map_err(|_| {
            RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                REFRESH_TOKEN_UNKNOWN_MESSAGE.to_string(),
            ))
        })?;
        if self.has_external_api_key_auth() {
            self.refresh_external_api_key().await?;
            return Ok(());
        }

        let Some(ForallAuth::Oauth(oauth)) = self.auth_cached() else {
            return Ok(());
        };
        let result = self.refresh_oauth_tokens(oauth.tokens()).await;
        match result {
            Ok(refreshed) => {
                // The server rotates the refresh token on every exchange and the
                // old one is single-use, so the new one must be persisted before
                // anything else can observe (and potentially retry with) it.
                save_oauth_tokens(
                    &self.forall_home,
                    refreshed.clone(),
                    self.auth_credentials_store_mode,
                    self.keyring_backend_kind,
                )
                .map_err(RefreshTokenError::Transient)?;
                self.set_cached_auth(Some(ForallAuth::Oauth(OauthAuth::new(refreshed))));
                Ok(())
            }
            Err(err) => {
                if let RefreshTokenError::Permanent(failed) = &err
                    && let Ok(mut guard) = self.inner.write()
                {
                    guard.permanent_refresh_failure = Some(AuthScopedRefreshFailure {
                        auth: ForallAuth::Oauth(oauth),
                        error: failed.clone(),
                    });
                }
                Err(err)
            }
        }
    }

    async fn refresh_oauth_tokens(
        &self,
        current: &TokenData,
    ) -> Result<TokenData, RefreshTokenError> {
        #[derive(serde::Deserialize)]
        struct OauthTokenResponse {
            id_token: String,
            access_token: String,
            refresh_token: String,
        }

        let response = crate::auth::default_client::credential_request(
            reqwest::Method::POST,
            &self.auth_base_urls.token_url(),
        )?
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|err| {
            RefreshTokenError::Transient(std::io::Error::other(format!(
                "failed to reach the Forall auth service: {err}"
            )))
        })?;

        let status = response.status();
        if status == reqwest::StatusCode::BAD_REQUEST || status == reqwest::StatusCode::UNAUTHORIZED
        {
            let body = response.text().await.unwrap_or_default();
            let reason = if body.contains("invalid_grant") {
                RefreshTokenFailedReason::Expired
            } else {
                RefreshTokenFailedReason::Other
            };
            return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                reason,
                format!("Authentication could not be refreshed (HTTP {status})."),
            )));
        }
        if !status.is_success() {
            return Err(RefreshTokenError::Transient(std::io::Error::other(
                format!("Forall auth service returned HTTP {status} during token refresh"),
            )));
        }

        let token_response: OauthTokenResponse = response.json().await.map_err(|err| {
            RefreshTokenError::Transient(std::io::Error::other(format!(
                "failed to parse Forall auth token response: {err}"
            )))
        })?;
        let id_token = crate::token_data::parse_chatgpt_jwt_claims(&token_response.id_token)
            .map_err(|err| {
                RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                    RefreshTokenFailedReason::Other,
                    format!("Forall auth service returned an invalid id token: {err}"),
                ))
            })?;
        let account_id = id_token
            .chatgpt_account_id
            .clone()
            .or_else(|| current.account_id.clone());
        Ok(TokenData {
            id_token,
            access_token: token_response.access_token,
            refresh_token: token_response.refresh_token,
            account_id,
        })
    }

    pub async fn refresh_token_from_authority(&self) -> Result<(), RefreshTokenError> {
        self.refresh_token().await
    }

    pub async fn logout(&self) -> std::io::Result<bool> {
        let removed = logout_all_stores(
            &self.forall_home,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )?;
        self.reload().await;
        Ok(removed)
    }

    pub async fn logout_with_revoke(&self) -> std::io::Result<bool> {
        let result = logout_all_stores(
            &self.forall_home,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )?;
        self.reload().await;
        Ok(result)
    }

    pub fn get_api_auth_mode(&self) -> Option<ApiAuthMode> {
        if self.has_external_api_key_auth() {
            return Some(ApiAuthMode::ApiKey);
        }
        self.auth_cached().as_ref().map(ForallAuth::api_auth_mode)
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        if self.has_external_api_key_auth() {
            return Some(AuthMode::ApiKey);
        }
        self.auth_cached().as_ref().map(ForallAuth::auth_mode)
    }

    pub fn current_auth_uses_forall_backend(&self) -> bool {
        self.auth_cached()
            .is_some_and(|auth| auth.uses_forall_backend())
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
