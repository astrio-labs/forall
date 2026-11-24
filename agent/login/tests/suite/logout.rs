use anyhow::Result;
use forall_app_server_protocol::AuthMode;
use forall_config::types::AuthCredentialsStoreMode;
use forall_login::AuthDotJson;
use forall_login::AuthKeyringBackendKind;
use forall_login::AuthManager;
use forall_login::login_with_api_key;
use forall_login::logout;
use forall_login::logout_with_revoke;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn api_key_auth(api_key: &str) -> AuthDotJson {
    AuthDotJson {
        provider_keys: Default::default(),
        preferred_provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(api_key.to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    }
}

#[tokio::test]
async fn logout_removes_api_key_auth_file() -> Result<()> {
    let forall_home = TempDir::new()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let removed = logout(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    assert!(removed);
    assert!(!forall_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn logout_with_revoke_removes_api_key_auth_file() -> Result<()> {
    let forall_home = TempDir::new()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let removed = logout_with_revoke(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await?;

    assert!(removed);
    assert!(!forall_home.path().join("auth.json").exists());
    Ok(())
}

#[tokio::test]
async fn auth_manager_logout_with_revoke_clears_cached_auth() -> Result<()> {
    let forall_home = TempDir::new()?;
    login_with_api_key(
        forall_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let manager = AuthManager::new(
        forall_home.path().to_path_buf(),
        /*enable_forall_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
        /*auth_base_urls*/ None,
    )
    .await;

    let removed = manager.logout_with_revoke().await?;

    assert!(removed);
    assert!(manager.auth_cached().is_none());
    assert!(!forall_home.path().join("auth.json").exists());
    Ok(())
}

#[test]
fn api_key_auth_round_trips_through_storage_helpers() -> Result<()> {
    let forall_home = TempDir::new()?;
    forall_login::save_auth(
        forall_home.path(),
        &api_key_auth("sk-test-key"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let loaded = forall_login::load_auth_dot_json(
        forall_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    assert_eq!(loaded, Some(api_key_auth("sk-test-key")));
    Ok(())
}
