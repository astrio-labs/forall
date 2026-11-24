use forall_app_server_protocol::AuthMode;
use forall_config::types::AuthCredentialsStoreMode;
use forall_protocol::config_types::ForcedLoginMethod;
use pretty_assertions::assert_eq;
use serial_test::serial;
use tempfile::tempdir;

use super::*;
use crate::auth::AuthKeyringBackendKind;
use crate::auth::AuthManager;
use crate::auth::ForallAuth;
use crate::auth::storage::AuthStorageBackend;
use crate::auth::storage::FileAuthStorage;

fn api_key_auth_dot_json(api_key: &str) -> AuthDotJson {
    AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(api_key.to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    }
}

#[test]
fn forall_auth_from_auth_dot_json_prefers_bedrock() {
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-openai".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: Some(BedrockApiKeyAuth {
            api_key: "bedrock-key".to_string(),
            region: "us-west-2".to_string(),
        }),
        github_connection: None,
    };

    let auth = ForallAuth::from_auth_dot_json(auth_dot_json).expect("bedrock auth");
    assert_eq!(auth.auth_mode(), AuthMode::BedrockApiKey);
}

#[test]
fn forall_auth_from_auth_dot_json_uses_openai_key() {
    let auth =
        ForallAuth::from_auth_dot_json(api_key_auth_dot_json("sk-openai")).expect("api key auth");
    assert_eq!(auth.auth_mode(), AuthMode::ApiKey);
    assert_eq!(auth.api_key(), Some("sk-openai"));
}

#[test]
fn forall_auth_from_auth_dot_json_uses_openrouter_key() {
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: None,
        openai_api_key: None,
        openrouter_api_key: Some("sk-or".to_string()),
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };

    let auth = ForallAuth::from_auth_dot_json(auth_dot_json).expect("openrouter auth");
    assert_eq!(auth.auth_mode(), AuthMode::ApiKey);
    assert_eq!(auth.api_key(), Some("sk-or"));
}

#[test]
fn forall_auth_from_auth_dot_json_uses_oauth_tokens() {
    let tokens = crate::token_data::TokenData {
        id_token: Default::default(),
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        account_id: Some("account".to_string()),
    };
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: None,
        // OAuth tokens win over stored BYOK keys.
        openai_api_key: Some("sk-openai".to_string()),
        openrouter_api_key: None,
        tokens: Some(tokens.clone()),
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };

    let auth = ForallAuth::from_auth_dot_json(auth_dot_json).expect("oauth auth");
    assert_eq!(auth.auth_mode(), AuthMode::Oauth);
    assert!(auth.is_forall_oauth());
    assert!(auth.uses_forall_backend());
    assert!(auth.identity_auth_available());
    assert_eq!(auth.api_key(), None);
    assert_eq!(auth.get_token().ok().as_deref(), Some("access"));
    assert_eq!(auth.get_token_data().ok(), Some(tokens));
    assert_eq!(auth.get_account_id().as_deref(), Some("account"));
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn login_with_api_key_persists_and_loads() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        /*auth_base_urls*/ None,
    )
    .await;

    assert_eq!(auth_manager.auth_mode(), Some(AuthMode::ApiKey));
    assert_eq!(
        auth_manager
            .auth_cached()
            .as_ref()
            .and_then(|auth| auth.api_key()),
        Some("sk-test-key")
    );
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn logout_removes_api_key_auth() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        /*auth_base_urls*/ None,
    )
    .await;

    assert!(auth_manager.logout().await?);
    assert_eq!(auth_manager.auth_cached(), None);
    assert!(!forall_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn enforce_login_restrictions_keeps_byok_auth() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let config = AuthConfig {
        forall_home: forall_home.path().to_path_buf(),
        auth_credentials_store_mode: AuthCredentialsStoreMode::File,
        keyring_backend_kind: AuthKeyringBackendKind::default(),
        forced_login_method: Some(ForcedLoginMethod::Api),
        auth_route_config: None,
    };

    enforce_login_restrictions(&config).await?;
    assert!(
        load_auth_dot_json(
            forall_home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )?
        .is_some()
    );
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn set_openai_api_key_updates_existing_auth() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    login_with_api_key(
        forall_home.path(),
        "sk-old",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    set_openai_api_key(
        forall_home.path(),
        "sk-new",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let loaded = FileAuthStorage::new(forall_home.path().to_path_buf()).load()?;
    assert_eq!(loaded, Some(api_key_auth_dot_json("sk-new")));
    Ok(())
}

#[test]
fn provider_key_never_falls_back_across_providers() {
    let mut auth = api_key_auth_dot_json("sk-openai");
    auth.openrouter_api_key = Some("sk-or".to_string());

    assert_eq!(auth.provider_key(OPENAI_API_KEY_ENV_VAR), Some("sk-openai"));
    assert_eq!(auth.provider_key(OPENROUTER_API_KEY_ENV_VAR), Some("sk-or"));
    // An env key with no stored slot must never resolve to another provider's key.
    assert_eq!(auth.provider_key("ANTHROPIC_API_KEY"), None);
    assert_eq!(auth.provider_key("GEMINI_API_KEY"), None);
}

#[test]
fn set_provider_key_isolates_unknown_env_keys() {
    let mut auth = AuthDotJson::default();
    auth.set_provider_key("ANTHROPIC_API_KEY", "sk-ant".to_string());

    assert_eq!(auth.openai_api_key, None);
    assert_eq!(auth.openrouter_api_key, None);
    assert_eq!(auth.provider_key("ANTHROPIC_API_KEY"), Some("sk-ant"));

    // Round-trips through serde under the provider_keys map, not a legacy field.
    let json = serde_json::to_value(&auth).expect("serialize");
    assert_eq!(json["provider_keys"]["ANTHROPIC_API_KEY"], "sk-ant");
    let restored: AuthDotJson = serde_json::from_value(json).expect("deserialize");
    assert_eq!(restored, auth);
}

#[test]
fn set_byok_api_key_rejects_unknown_provider_and_preserves_existing_keys() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    set_byok_api_key(
        forall_home.path(),
        "openai",
        "sk-openai",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let err = set_byok_api_key(
        forall_home.path(),
        "mystery-provider",
        "sk-mystery",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )
    .expect_err("unknown provider must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

    // The rejected write must not have touched any other provider's slot.
    let loaded = FileAuthStorage::new(forall_home.path().to_path_buf())
        .load()?
        .expect("auth.json present");
    assert_eq!(loaded.openai_api_key.as_deref(), Some("sk-openai"));
    assert!(loaded.provider_keys.is_empty());
    Ok(())
}

#[test]
fn set_byok_api_key_round_trips_azure_anthropic_gemini_into_provider_keys() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    set_byok_api_key(
        forall_home.path(),
        "openai",
        "sk-openai",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    for (provider, env_key, api_key) in [
        ("azure", "AZURE_OPENAI_API_KEY", "azure-key"),
        ("anthropic", "ANTHROPIC_API_KEY", "sk-ant"),
        ("gemini", "GEMINI_API_KEY", "gemini-key"),
    ] {
        set_byok_api_key(
            forall_home.path(),
            provider,
            api_key,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )?;

        let loaded = FileAuthStorage::new(forall_home.path().to_path_buf())
            .load()?
            .expect("auth.json present");
        // Stored under provider_keys, never a legacy slot.
        assert_eq!(
            loaded.provider_keys.get(env_key).map(String::as_str),
            Some(api_key)
        );
        assert_eq!(loaded.provider_key(env_key), Some(api_key));
        // The OpenAI slot is untouched by other providers' writes.
        assert_eq!(loaded.openai_api_key.as_deref(), Some("sk-openai"));
        assert_eq!(loaded.openrouter_api_key, None);
    }
    Ok(())
}

#[test]
fn read_stored_provider_api_key_from_disk_does_not_cross_providers() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    set_byok_api_key(
        forall_home.path(),
        "openai",
        "sk-openai",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    // Regression: an unknown env key used to fall back to the stored OpenAI key,
    // leaking it to a different provider's endpoint.
    assert_eq!(
        read_stored_provider_api_key_from_disk(
            forall_home.path(),
            "ANTHROPIC_API_KEY",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        ),
        None
    );
    assert_eq!(
        read_stored_provider_api_key_from_disk(
            forall_home.path(),
            OPENAI_API_KEY_ENV_VAR,
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        ),
        Some("sk-openai".to_string())
    );
    Ok(())
}

#[test]
fn provider_keys_round_trip_through_storage_and_memory() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mut auth = api_key_auth_dot_json("sk-openai");
    auth.set_provider_key("ANTHROPIC_API_KEY", "sk-ant".to_string());
    FileAuthStorage::new(forall_home.path().to_path_buf()).save(&auth)?;

    assert_eq!(
        read_stored_provider_api_key_from_disk(
            forall_home.path(),
            "ANTHROPIC_API_KEY",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        ),
        Some("sk-ant".to_string())
    );

    let in_memory = ForallAuth::from_auth_dot_json(auth).expect("api key auth");
    assert_eq!(
        in_memory.stored_provider_env_api_key("ANTHROPIC_API_KEY"),
        Some("sk-ant".to_string())
    );
    assert_eq!(
        in_memory.stored_provider_env_api_key("GEMINI_API_KEY"),
        None
    );
    Ok(())
}

#[test]
fn auth_dot_json_without_provider_keys_field_parses() {
    // Legacy auth.json written before the provider_keys map existed.
    let legacy = r#"{"OPENAI_API_KEY":"sk-legacy"}"#;
    let auth: AuthDotJson = serde_json::from_str(legacy).expect("legacy auth.json parses");
    assert_eq!(auth.openai_api_key.as_deref(), Some("sk-legacy"));
    assert!(auth.provider_keys.is_empty());
}

#[test]
fn map_only_provider_keys_still_produce_api_key_auth() {
    // Regression (PR #67 review): auth.json holding only non-legacy provider
    // keys must still construct in-memory auth so resolve_provider_auth can
    // find those keys without the disk fallback.
    let mut auth_dot_json = AuthDotJson::default();
    auth_dot_json.set_provider_key("ANTHROPIC_API_KEY", "sk-ant".to_string());

    let auth = ForallAuth::from_auth_dot_json(auth_dot_json).expect("map-only auth");
    assert_eq!(auth.auth_mode(), AuthMode::ApiKey);
    assert_eq!(
        auth.stored_provider_env_api_key("ANTHROPIC_API_KEY"),
        Some("sk-ant".to_string())
    );
    // No primary bearer key: it must never fall back to another provider's key.
    assert_eq!(auth.api_key(), None);
    assert!(auth.get_token().is_err());
}

#[test]
fn empty_auth_dot_json_still_produces_no_auth() {
    assert_eq!(ForallAuth::from_auth_dot_json(AuthDotJson::default()), None);
}

fn fake_forall_jwt(plan: &str, account_id: &str, user_id: &str, email: &str) -> String {
    use base64::Engine as _;

    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header = serde_json::json!({ "alg": "RS256", "typ": "JWT" });
    let payload = serde_json::json!({
        "email": email,
        "https://forall.astrio.app/auth": {
            "chatgpt_plan_type": plan,
            "chatgpt_account_id": account_id,
            "chatgpt_user_id": user_id,
        },
    });
    let header_b64 = encode(&serde_json::to_vec(&header).expect("serialize header"));
    let payload_b64 = encode(&serde_json::to_vec(&payload).expect("serialize payload"));
    let signature_b64 = encode(b"sig");
    format!("{header_b64}.{payload_b64}.{signature_b64}")
}

fn oauth_token_data(jwt: &str, refresh_token: &str) -> TokenData {
    TokenData {
        id_token: crate::token_data::parse_chatgpt_jwt_claims(jwt).expect("jwt parses"),
        access_token: jwt.to_string(),
        refresh_token: refresh_token.to_string(),
        account_id: None,
    }
}

/// Serves a single canned response on a loopback tiny_http server and returns
/// (base_url, join handle yielding the observed request url + body).
fn spawn_one_shot_token_server(
    status: u16,
    body: String,
) -> (String, std::thread::JoinHandle<(String, String)>) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind loopback server");
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
        #[cfg(unix)]
        tiny_http::ListenAddr::Unix(_) => unreachable!("loopback server uses an IP address"),
    };
    let handle = std::thread::spawn(move || {
        let mut request = server.recv().expect("receive token request");
        let url = request.url().to_string();
        let mut request_body = String::new();
        request
            .as_reader()
            .read_to_string(&mut request_body)
            .expect("read request body");
        let response = tiny_http::Response::from_string(body).with_status_code(status);
        request.respond(response).expect("send token response");
        (url, request_body)
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn save_oauth_tokens_round_trips_to_oauth_auth() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    // Pre-existing BYOK key must survive an OAuth login.
    set_openai_api_key(
        forall_home.path(),
        "sk-byok",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let jwt = fake_forall_jwt("pro", "acct-1", "user-1", "user@example.com");
    save_oauth_tokens(
        forall_home.path(),
        oauth_token_data(&jwt, "refresh-1"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        /*auth_base_urls*/ None,
    )
    .await;

    assert_eq!(auth_manager.auth_mode(), Some(AuthMode::Oauth));
    assert!(auth_manager.current_auth_uses_forall_backend());
    let auth = auth_manager.auth_cached().expect("oauth auth cached");
    assert!(auth.is_forall_oauth());
    assert_eq!(auth.get_token().ok(), Some(jwt.clone()));
    assert_eq!(
        auth.account_plan_type(),
        Some(forall_protocol::account::PlanType::Pro)
    );
    assert_eq!(
        auth.get_account_email().as_deref(),
        Some("user@example.com")
    );
    assert_eq!(auth.get_chatgpt_user_id().as_deref(), Some("user-1"));
    assert_eq!(auth.get_account_id().as_deref(), Some("acct-1"));

    let stored = load_auth_dot_json(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?
    .expect("auth.json present");
    assert_eq!(stored.auth_mode, Some(AuthMode::Oauth));
    assert_eq!(stored.openai_api_key.as_deref(), Some("sk-byok"));
    assert!(stored.last_refresh.is_some());
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn refresh_token_persists_rotated_refresh_token() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let old_jwt = fake_forall_jwt("free", "acct-1", "user-1", "user@example.com");
    save_oauth_tokens(
        forall_home.path(),
        oauth_token_data(&old_jwt, "refresh-old"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let new_jwt = fake_forall_jwt("plus", "acct-1", "user-1", "user@example.com");
    let response = serde_json::json!({
        "id_token": new_jwt.clone(),
        "access_token": new_jwt.clone(),
        "refresh_token": "refresh-new",
        "token_type": "Bearer",
        "expires_in": 3600,
    });
    let (base_url, server) = spawn_one_shot_token_server(200, response.to_string());

    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        Some(AuthBaseUrls::resolve(Some(&base_url))),
    )
    .await;

    auth_manager.refresh_token().await?;

    let (url, request_body) = server.join().expect("token server thread");
    assert_eq!(url, "/oauth/token");
    assert!(request_body.contains("grant_type=refresh_token"));
    assert!(request_body.contains("refresh_token=refresh-old"));

    // The rotated refresh token must be persisted immediately: the old one is
    // single-use and replaying it revokes the whole token family.
    let stored = load_auth_dot_json(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?
    .expect("auth.json present");
    let stored_tokens = stored.tokens.expect("tokens persisted");
    assert_eq!(stored_tokens.refresh_token, "refresh-new");
    assert_eq!(stored_tokens.access_token, new_jwt);

    let auth = auth_manager.auth_cached().expect("oauth auth cached");
    assert_eq!(auth.get_token().ok(), Some(new_jwt));
    assert_eq!(
        auth.account_plan_type(),
        Some(forall_protocol::account::PlanType::Plus)
    );
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn refresh_token_invalid_grant_is_permanent() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let jwt = fake_forall_jwt("free", "acct-1", "user-1", "user@example.com");
    save_oauth_tokens(
        forall_home.path(),
        oauth_token_data(&jwt, "refresh-revoked"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let (base_url, server) =
        spawn_one_shot_token_server(400, r#"{"error":"invalid_grant"}"#.to_string());

    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        Some(AuthBaseUrls::resolve(Some(&base_url))),
    )
    .await;

    let err = auth_manager
        .refresh_token()
        .await
        .expect_err("invalid_grant must fail");
    assert_eq!(err.failed_reason(), Some(RefreshTokenFailedReason::Expired));
    server.join().expect("token server thread");

    // The stale refresh token stays stored; nothing was rotated.
    let stored = load_auth_dot_json(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?
    .expect("auth.json present");
    assert_eq!(
        stored.tokens.expect("tokens present").refresh_token,
        "refresh-revoked"
    );
    Ok(())
}

#[tokio::test]
#[serial(forall_auth_env)]
async fn refresh_token_is_noop_for_byok_api_key_auth() -> anyhow::Result<()> {
    // Regression: pure BYOK users must behave exactly as before OAuth existed.
    let forall_home = tempdir()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let auth_manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        /*auth_base_urls*/ None,
    )
    .await;

    auth_manager.refresh_token().await?;
    assert_eq!(auth_manager.auth_mode(), Some(AuthMode::ApiKey));
    assert!(!auth_manager.current_auth_uses_forall_backend());
    Ok(())
}

#[test]
fn auth_base_urls_resolve_prefers_config_and_joins_paths() {
    let urls = AuthBaseUrls::resolve(Some("https://auth.example.com/"));
    assert_eq!(urls.base_url(), "https://auth.example.com");
    assert_eq!(
        urls.authorize_url(),
        "https://auth.example.com/oauth/authorize"
    );
    assert_eq!(urls.token_url(), "https://auth.example.com/oauth/token");
    assert_eq!(urls.revoke_url(), "https://auth.example.com/oauth/revoke");

    let default_urls = AuthBaseUrls::resolve(None);
    assert_eq!(default_urls.base_url(), DEFAULT_FORALL_AUTH_BASE_URL);
}
