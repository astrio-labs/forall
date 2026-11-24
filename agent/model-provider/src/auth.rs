use std::sync::Arc;

use forall_api::AuthProvider;
use forall_api::SharedAuthProvider;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_login::OPENAI_API_KEY_ENV_VAR;
use forall_model_provider_info::ModelProviderInfo;
use forall_protocol::error::ForallErr;
use http::HeaderMap;
use http::HeaderName;

use crate::bearer_auth_provider::BearerAuthProvider;
use crate::header_auth_provider::HeaderAuthProvider;

const BEDROCK_API_KEY_UNSUPPORTED_MESSAGE: &str =
    "Bedrock API key auth is only supported by the Amazon Bedrock model provider";

const FORALL_AUTH_REQUIRED_MESSAGE: &str =
    "This provider requires a Forall account. Sign in with: forall login";

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

/// Returns the provider-scoped auth manager when this provider uses command-backed auth.
///
/// Providers without custom auth continue using the caller-supplied base manager, when present.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None => auth_manager,
    }
}

pub(crate) fn resolve_provider_auth_with_manager(
    auth: Option<&ForallAuth>,
    provider: &ModelProviderInfo,
    auth_manager: Option<&AuthManager>,
) -> forall_protocol::error::Result<SharedAuthProvider> {
    if matches!(auth, Some(ForallAuth::BedrockApiKey(_))) {
        return Err(ForallErr::UnsupportedOperation(
            BEDROCK_API_KEY_UNSUPPORTED_MESSAGE.to_string(),
        ));
    }

    // Providers backed by the user's Forall account (the built-in `astrio`
    // provider) authenticate exclusively with the Forall OAuth access token.
    // API keys — from the environment, auth.json, or bearer overrides — are
    // never used for these providers.
    if provider.requires_forall_auth {
        return match auth {
            Some(auth @ ForallAuth::Oauth(_)) => Ok(auth_provider_from_auth(auth)),
            _ => Err(ForallErr::InvalidRequest(
                FORALL_AUTH_REQUIRED_MESSAGE.to_string(),
            )),
        };
    }

    // BYOK providers (OpenRouter, Azure, etc.) must use their own API key — never a
    // Forall JWT or an OpenAI key from a different provider.
    if provider.env_key.is_some() {
        if let Some(provider_auth) = bearer_auth_for_provider(provider, auth, auth_manager)? {
            return Ok(provider_auth);
        }
        return Ok(unauthenticated_auth_provider());
    }

    if let Some(provider_auth) = bearer_auth_for_provider(provider, auth, auth_manager)? {
        return Ok(provider_auth);
    }

    // Prefer the auth-aware check over the BYOK-only free function: identity
    // auth is available exactly when the active auth is Forall OAuth.
    let identity_auth_disabled = !auth.is_some_and(ForallAuth::identity_auth_available);

    // The OpenAI provider authenticates with OPENAI_API_KEY from the
    // environment (or auth.json). An explicit key wins even when a Forall
    // account is signed in: direct provider access is exactly what a stored
    // key asks for, and the Forall token is not valid at api.openai.com.
    if provider.is_openai()
        && let Some(api_key) = read_non_empty_env_var(OPENAI_API_KEY_ENV_VAR)
            .or_else(|| {
                auth.and_then(|auth| auth.stored_provider_env_api_key(OPENAI_API_KEY_ENV_VAR))
            })
            .or_else(|| {
                auth_manager.and_then(|manager| {
                    manager.stored_env_api_key_from_disk(OPENAI_API_KEY_ENV_VAR)
                })
            })
    {
        return Ok(Arc::new(BearerAuthProvider::new(api_key)));
    }

    if identity_auth_disabled && (provider.env_key.is_some() || !provider.requires_openai_auth) {
        return Ok(unauthenticated_auth_provider());
    }

    match auth {
        // A Forall access token authenticates the user's Forall account and
        // is only ever valid at the hosted gateway (`requires_forall_auth`
        // providers, handled above). Sending it anywhere else would leak the
        // account credential to a third-party host — which rejects it anyway
        // (OpenAI answers 401 invalid_issuer).
        Some(ForallAuth::Oauth(_)) => Err(ForallErr::InvalidRequest(format!(
            "Model provider `{}` needs its own API key. Your Forall account runs hosted \
             models through the `astrio` provider; to call `{}` directly, add its API key \
             in settings (or ~/.forall/.env).",
            provider.name, provider.name
        ))),
        Some(auth) => Ok(auth_provider_from_auth(auth)),
        None => Ok(unauthenticated_auth_provider()),
    }
}

fn provider_env_api_key(
    provider: &ModelProviderInfo,
    auth: Option<&ForallAuth>,
    auth_manager: Option<&AuthManager>,
) -> Option<String> {
    let env_key = provider.env_key.as_ref()?;
    read_non_empty_env_var(env_key)
        .or_else(|| auth.and_then(|auth| auth.stored_provider_env_api_key(env_key)))
        .or_else(|| auth_manager.and_then(|manager| manager.stored_env_api_key_from_disk(env_key)))
}

fn read_non_empty_env_var(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
    auth: Option<&ForallAuth>,
    auth_manager: Option<&AuthManager>,
) -> forall_protocol::error::Result<Option<SharedAuthProvider>> {
    if let Some(api_key) = provider_env_api_key(provider, auth, auth_manager) {
        return Ok(Some(api_key_auth_provider(provider, api_key)?));
    }

    // `experimental_bearer_token` is by definition a bearer token, so it keeps
    // the `Authorization: Bearer` header regardless of `auth_header`.
    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(Arc::new(BearerAuthProvider::new(token))));
    }

    Ok(None)
}

/// Wraps a resolved provider API key in the provider's auth scheme.
///
/// When `auth_header` is set, the key is sent verbatim in that header (e.g.
/// Azure OpenAI's `api-key`); otherwise the standard `Authorization: Bearer`
/// header is used.
fn api_key_auth_provider(
    provider: &ModelProviderInfo,
    api_key: String,
) -> forall_protocol::error::Result<SharedAuthProvider> {
    match provider.auth_header.as_deref() {
        Some(auth_header) => {
            let header_name = HeaderName::try_from(auth_header).map_err(|_| {
                ForallErr::InvalidRequest(format!(
                    "provider {name} has an invalid auth_header: `{auth_header}`",
                    name = provider.name
                ))
            })?;
            Ok(Arc::new(HeaderAuthProvider::new(header_name, api_key)))
        }
        None => Ok(Arc::new(BearerAuthProvider::new(api_key))),
    }
}

/// Builds request-header auth for a first-party Forall auth snapshot.
pub fn auth_provider_from_auth(auth: &ForallAuth) -> SharedAuthProvider {
    match auth {
        ForallAuth::BedrockApiKey(_) => unreachable!("{BEDROCK_API_KEY_UNSUPPORTED_MESSAGE}"),
        ForallAuth::ApiKey(_) | ForallAuth::Oauth(_) => Arc::new(BearerAuthProvider {
            token: auth.get_token().ok(),
            account_id: auth.get_account_id(),
            is_fedramp_account: auth.is_fedramp_account(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use forall_login::auth::BedrockApiKeyAuth;
    use forall_model_provider_info::WireApi;
    use forall_model_provider_info::create_oss_provider_with_base_url;
    use pretty_assertions::assert_eq;

    use super::*;

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn unset(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation restored on drop.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restore the original environment value before other tests run.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial_test::serial(openrouter_env)]
    fn openrouter_provider_uses_stored_openrouter_key_from_disk() {
        use forall_login::AuthManager;

        let _env_guard = EnvGuard::unset("OPENROUTER_API_KEY");
        let provider = ModelProviderInfo::create_openrouter_provider();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"OPENROUTER_API_KEY":"sk-or-test","OPENAI_API_KEY":"sk-openai"}"#,
        )
        .unwrap();
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            ForallAuth::from_api_key("sk-openai"),
            dir.path().to_path_buf(),
        );
        let resolved = resolve_provider_auth_with_manager(
            auth_manager.auth_cached().as_ref(),
            &provider,
            Some(auth_manager.as_ref()),
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-or-test")
        );
    }

    struct EnvSetGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvSetGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation restored on drop.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvSetGuard {
        fn drop(&mut self) {
            // SAFETY: restore the original environment value before other tests run.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    #[serial_test::serial(openai_env)]
    fn openai_provider_uses_env_api_key_without_identity_auth() {
        let _env_guard = EnvSetGuard::set("OPENAI_API_KEY", "sk-openai-env");
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

        let resolved = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-openai-env")
        );
    }

    #[test]
    #[serial_test::serial(azure_env)]
    fn azure_provider_uses_api_key_header_instead_of_bearer() {
        let _env_guard = EnvSetGuard::set("AZURE_OPENAI_API_KEY", "azure-env-key");
        let provider = ModelProviderInfo::create_azure_provider();

        let resolved = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("api-key").and_then(|v| v.to_str().ok()),
            Some("azure-env-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(azure_env)]
    fn azure_provider_uses_stored_azure_key_from_disk_in_api_key_header() {
        use forall_login::AuthManager;

        let _env_guard = EnvGuard::unset("AZURE_OPENAI_API_KEY");
        let provider = ModelProviderInfo::create_azure_provider();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-openai","provider_keys":{"AZURE_OPENAI_API_KEY":"azure-stored-key"}}"#,
        )
        .unwrap();
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            ForallAuth::from_api_key("sk-openai"),
            dir.path().to_path_buf(),
        );

        let resolved = resolve_provider_auth_with_manager(
            auth_manager.auth_cached().as_ref(),
            &provider,
            Some(auth_manager.as_ref()),
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("api-key").and_then(|v| v.to_str().ok()),
            Some("azure-stored-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(anthropic_env)]
    fn anthropic_provider_uses_x_api_key_header_instead_of_bearer() {
        let _env_guard = EnvSetGuard::set("ANTHROPIC_API_KEY", "anthropic-env-key");
        let provider = ModelProviderInfo::create_anthropic_provider();

        let resolved = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("anthropic-env-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(gemini_env)]
    fn gemini_provider_uses_x_goog_api_key_header_instead_of_bearer() {
        let _env_guard = EnvSetGuard::set("GEMINI_API_KEY", "gemini-env-key");
        let provider = ModelProviderInfo::create_gemini_provider();

        let resolved = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()),
            Some("gemini-env-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(gemini_env)]
    fn gemini_provider_uses_stored_gemini_key_from_disk_in_x_goog_api_key_header() {
        use forall_login::AuthManager;

        let _env_guard = EnvGuard::unset("GEMINI_API_KEY");
        let provider = ModelProviderInfo::create_gemini_provider();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-openai","provider_keys":{"GEMINI_API_KEY":"gemini-stored-key"}}"#,
        )
        .unwrap();
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            ForallAuth::from_api_key("sk-openai"),
            dir.path().to_path_buf(),
        );

        let resolved = resolve_provider_auth_with_manager(
            auth_manager.auth_cached().as_ref(),
            &provider,
            Some(auth_manager.as_ref()),
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()),
            Some("gemini-stored-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(anthropic_env)]
    fn anthropic_provider_uses_stored_anthropic_key_from_disk_in_x_api_key_header() {
        use forall_login::AuthManager;

        let _env_guard = EnvGuard::unset("ANTHROPIC_API_KEY");
        let provider = ModelProviderInfo::create_anthropic_provider();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-openai","provider_keys":{"ANTHROPIC_API_KEY":"anthropic-stored-key"}}"#,
        )
        .unwrap();
        let auth_manager = AuthManager::from_auth_for_testing_with_home(
            ForallAuth::from_api_key("sk-openai"),
            dir.path().to_path_buf(),
        );

        let resolved = resolve_provider_auth_with_manager(
            auth_manager.auth_cached().as_ref(),
            &provider,
            Some(auth_manager.as_ref()),
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers.get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("anthropic-stored-key")
        );
        assert!(headers.get(http::header::AUTHORIZATION).is_none());
    }

    #[test]
    #[serial_test::serial(openrouter_env)]
    fn openrouter_provider_keeps_bearer_authorization_header() {
        let _env_guard = EnvSetGuard::set("OPENROUTER_API_KEY", "sk-or-env");
        let provider = ModelProviderInfo::create_openrouter_provider();

        let resolved = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-or-env")
        );
        assert!(headers.get("api-key").is_none());
    }

    #[test]
    fn unauthenticated_auth_provider_adds_no_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let auth = resolve_provider_auth_with_manager(
            /*auth*/ None, &provider, /*auth_manager*/ None,
        )
        .expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    fn oauth_auth(access_token: &str) -> ForallAuth {
        ForallAuth::Oauth(forall_login::OauthAuth::new(forall_login::TokenData {
            id_token: Default::default(),
            access_token: access_token.to_string(),
            refresh_token: "refresh".to_string(),
            account_id: None,
        }))
    }

    #[test]
    fn astrio_provider_uses_forall_oauth_access_token() {
        let provider = ModelProviderInfo::create_astrio_provider();
        let auth = oauth_auth("forall-access-token");

        let resolved =
            resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None)
                .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer forall-access-token")
        );
    }

    #[test]
    fn astrio_provider_without_oauth_returns_login_error() {
        let provider = ModelProviderInfo::create_astrio_provider();

        for auth in [None, Some(ForallAuth::from_api_key("sk-openai"))] {
            match resolve_provider_auth_with_manager(
                auth.as_ref(),
                &provider,
                /*auth_manager*/ None,
            ) {
                Err(ForallErr::InvalidRequest(message)) => {
                    assert_eq!(message, FORALL_AUTH_REQUIRED_MESSAGE);
                    assert!(message.contains("forall login"));
                }
                Err(err) => panic!("unexpected auth error: {err:?}"),
                Ok(_) => panic!("astrio auth should require Forall OAuth"),
            }
        }
    }

    #[test]
    fn oauth_token_never_leaks_to_the_openai_provider() {
        let _env_guard = EnvGuard::unset("OPENAI_API_KEY");
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let auth = oauth_auth("forall-access-token");

        match resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None) {
            Err(ForallErr::InvalidRequest(message)) => {
                assert!(message.contains("needs its own API key"), "{message}");
                assert!(message.contains("astrio"), "{message}");
            }
            Err(err) => panic!("unexpected auth error: {err:?}"),
            Ok(_) => panic!("a Forall token must never authenticate a non-forall provider"),
        }
    }

    #[test]
    fn openai_env_key_wins_over_forall_oauth() {
        let _env_guard = EnvSetGuard::set("OPENAI_API_KEY", "sk-explicit");
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let auth = oauth_auth("forall-access-token");

        let resolved =
            resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None)
                .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-explicit")
        );
    }

    #[test]
    #[serial_test::serial(openrouter_env)]
    fn byok_provider_with_oauth_present_still_uses_own_env_key() {
        let _env_guard = EnvSetGuard::set("OPENROUTER_API_KEY", "sk-or-env");
        let provider = ModelProviderInfo::create_openrouter_provider();
        let auth = oauth_auth("forall-access-token");

        let resolved =
            resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None)
                .expect("auth should resolve");
        let headers = resolved.to_auth_headers();
        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer sk-or-env")
        );
    }

    #[test]
    #[serial_test::serial(openrouter_env)]
    fn byok_provider_with_oauth_and_no_env_key_never_uses_oauth_token() {
        let _env_guard = EnvGuard::unset("OPENROUTER_API_KEY");
        let provider = ModelProviderInfo::create_openrouter_provider();
        let auth = oauth_auth("forall-access-token");

        let resolved =
            resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None)
                .expect("auth should resolve");

        assert!(resolved.to_auth_headers().is_empty());
    }

    #[test]
    fn openai_provider_rejects_bedrock_api_key_auth() {
        let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        let auth = ForallAuth::BedrockApiKey(BedrockApiKeyAuth {
            api_key: "bedrock-api-key-test".to_string(),
            region: "us-east-1".to_string(),
        });

        match resolve_provider_auth_with_manager(Some(&auth), &provider, /*auth_manager*/ None) {
            Err(ForallErr::UnsupportedOperation(message)) => {
                assert_eq!(message, BEDROCK_API_KEY_UNSUPPORTED_MESSAGE);
            }
            Err(err) => panic!("unexpected auth error: {err:?}"),
            Ok(_) => panic!("Bedrock API key auth should be rejected"),
        }
    }
}
