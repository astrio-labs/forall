use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use forall_app_server_protocol::AuthMode;
use forall_config::types::AuthCredentialsStoreMode;
use forall_login::AuthDotJson;
use forall_login::AuthKeyringBackendKind;
use forall_login::TokenData;
use forall_login::save_auth;
use forall_login::token_data::parse_chatgpt_jwt_claims;

/// Builder for writing authenticated auth.json in integration tests.
#[derive(Debug, Clone)]
pub struct ApiKeyAuthFixture {
    access_token: String,
}

impl ApiKeyAuthFixture {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
        }
    }

    pub fn refresh_token(self, _refresh_token: impl Into<String>) -> Self {
        self
    }

    pub fn account_id(self, _account_id: impl Into<String>) -> Self {
        self
    }

    pub fn plan_type(self, _plan_type: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_user_id(self, _chatgpt_user_id: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_account_id(self, _chatgpt_account_id: impl Into<String>) -> Self {
        self
    }

    pub fn email(self, _email: impl Into<String>) -> Self {
        self
    }

    pub fn last_refresh(self, _last_refresh: Option<chrono::DateTime<chrono::Utc>>) -> Self {
        self
    }

    pub fn claims(self, _claims: LegacyIdTokenClaims) -> Self {
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct LegacyIdTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub chatgpt_user_id: Option<String>,
    pub chatgpt_account_id: Option<String>,
}

impl LegacyIdTokenClaims {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }

    pub fn plan_type(mut self, plan_type: impl Into<String>) -> Self {
        self.plan_type = Some(plan_type.into());
        self
    }

    pub fn chatgpt_user_id(mut self, chatgpt_user_id: impl Into<String>) -> Self {
        self.chatgpt_user_id = Some(chatgpt_user_id.into());
        self
    }

    pub fn chatgpt_account_id(mut self, chatgpt_account_id: impl Into<String>) -> Self {
        self.chatgpt_account_id = Some(chatgpt_account_id.into());
        self
    }
}

pub fn encode_id_token(_claims: &LegacyIdTokenClaims) -> Result<String> {
    Ok("legacy-id-token".to_string())
}

/// Builds an unsigned JWT-shaped id token carrying the Forall auth claims
/// namespace; signature verification is not performed by the client.
pub fn oauth_fixture_id_token() -> Result<String> {
    let payload = serde_json::json!({
        "email": "user@example.com",
        "https://forall.astrio.app/auth": {
            "chatgpt_user_id": "user-1",
            "chatgpt_account_id": "account-1",
            "chatgpt_plan_type": "pro",
        },
    });
    let payload_bytes = serde_json::to_vec(&payload)?;
    Ok(format!(
        "{}.{}.{}",
        URL_SAFE_NO_PAD.encode(b"{}"),
        URL_SAFE_NO_PAD.encode(payload_bytes),
        URL_SAFE_NO_PAD.encode(b"sig"),
    ))
}

/// Writes an auth.json holding managed Forall OAuth credentials, as produced
/// by a completed OAuth login.
pub fn write_oauth_auth(forall_home: &Path, access_token: &str, refresh_token: &str) -> Result<()> {
    let id_token_jwt = oauth_fixture_id_token()?;
    let tokens = TokenData {
        id_token: parse_chatgpt_jwt_claims(&id_token_jwt).context("parse fixture id token")?,
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        account_id: None,
    };

    let auth = AuthDotJson {
        provider_keys: Default::default(),
        preferred_provider_keys: Default::default(),
        auth_mode: Some(AuthMode::Oauth),
        openai_api_key: None,
        openrouter_api_key: None,
        tokens: Some(tokens),
        last_refresh: Some(chrono::Utc::now()),
        bedrock_api_key: None,
        github_connection: None,
    };

    save_auth(
        forall_home,
        &auth,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )
    .context("write oauth auth.json")
}

pub fn write_api_key_auth(
    forall_home: &Path,
    fixture: ApiKeyAuthFixture,
    cli_auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<()> {
    let api_key = if fixture.access_token.starts_with("sk-") {
        fixture.access_token
    } else {
        format!("sk-test-{}", fixture.access_token)
    };

    let auth = AuthDotJson {
        provider_keys: Default::default(),
        preferred_provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(api_key),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };

    save_auth(
        forall_home,
        &auth,
        cli_auth_credentials_store_mode,
        AuthKeyringBackendKind::default(),
    )
    .context("write auth.json")
}
