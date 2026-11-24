use std::path::Path;

use forall_config::types::AuthCredentialsStoreMode;
use serde::Deserialize;
use serde::Serialize;

use super::manager::save_auth;
use super::storage::AuthDotJson;
use super::storage::AuthKeyringBackendKind;
use forall_app_server_protocol::AuthMode;

/// Managed Amazon Bedrock API key persisted in `auth.json`.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct BedrockApiKeyAuth {
    pub api_key: String,
    pub region: String,
}

/// Writes an `auth.json` that contains only the Amazon Bedrock API key auth.
pub fn login_with_bedrock_api_key(
    forall_home: &Path,
    api_key: &str,
    region: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::BedrockApiKey),
        openai_api_key: None,
        openrouter_api_key: None,
        provider_keys: Default::default(),
        preferred_provider_keys: Default::default(),
        tokens: None,
        last_refresh: None,
        bedrock_api_key: Some(BedrockApiKeyAuth {
            api_key: api_key.to_string(),
            region: region.to_string(),
        }),
        github_connection: None,
    };
    save_auth(
        forall_home,
        &auth_dot_json,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

#[cfg(test)]
#[path = "bedrock_api_key_tests.rs"]
mod tests;
