use super::*;
use crate::token_data::IdTokenInfo;
use anyhow::Context;
use base64::Engine;
use forall_secrets::LocalSecretsNamespace;
use forall_secrets::SecretScope;
use forall_secrets::SecretsBackendKind;
use forall_secrets::SecretsManager;
use forall_secrets::compute_keyring_account;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

use forall_keyring_store::tests::MockKeyringStore;
use keyring::Error as KeyringError;

#[tokio::test]
async fn file_storage_load_returns_auth_dot_json() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let storage = FileAuthStorage::new(forall_home.path().to_path_buf());
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("test-key".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: Some(Utc::now()),
        bedrock_api_key: None,
        github_connection: None,
    };

    storage
        .save(&auth_dot_json)
        .context("failed to save auth file")?;

    let loaded = storage.load().context("failed to load auth file")?;
    assert_eq!(Some(auth_dot_json), loaded);
    Ok(())
}

#[tokio::test]
async fn file_storage_save_persists_auth_dot_json() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let storage = FileAuthStorage::new(forall_home.path().to_path_buf());
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("test-key".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: Some(Utc::now()),
        bedrock_api_key: None,
        github_connection: None,
    };

    let file = get_auth_file(forall_home.path());
    storage
        .save(&auth_dot_json)
        .context("failed to save auth file")?;

    let same_auth_dot_json = storage
        .try_read_auth_json(&file)
        .context("failed to read auth file after save")?;
    assert_eq!(auth_dot_json, same_auth_dot_json);
    Ok(())
}

#[tokio::test]
async fn file_storage_round_trips_auth_without_optional_fields() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let storage = FileAuthStorage::new(forall_home.path().to_path_buf());
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: None,
        openai_api_key: None,
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };

    storage.save(&auth_dot_json)?;

    let loaded = storage.load()?;
    assert_eq!(Some(auth_dot_json), loaded);
    Ok(())
}

#[test]
fn file_storage_delete_removes_auth_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-test-key".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };
    let storage = create_auth_storage(
        dir.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    );
    storage.save(&auth_dot_json)?;
    assert!(dir.path().join("auth.json").exists());
    let storage = FileAuthStorage::new(dir.path().to_path_buf());
    let removed = storage.delete()?;
    assert!(removed);
    assert!(!dir.path().join("auth.json").exists());
    Ok(())
}

#[test]
fn ephemeral_storage_save_load_delete_is_in_memory_only() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let storage = create_auth_storage(
        dir.path().to_path_buf(),
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::default(),
    );
    let auth_dot_json = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-ephemeral".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: Some(Utc::now()),
        bedrock_api_key: None,
        github_connection: None,
    };

    storage.save(&auth_dot_json)?;
    let loaded = storage.load()?;
    assert_eq!(Some(auth_dot_json), loaded);

    let removed = storage.delete()?;
    assert!(removed);
    let loaded = storage.load()?;
    assert_eq!(None, loaded);
    assert!(!get_auth_file(dir.path()).exists());
    Ok(())
}

fn seed_secrets_backend_and_fallback_auth_file_for_delete(
    mock_keyring: &MockKeyringStore,
    forall_home: &Path,
    auth: &AuthDotJson,
) -> anyhow::Result<PathBuf> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        forall_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::ForallAuth,
    );
    manager.set(
        &SecretScope::Global,
        &CODEX_AUTH_SECRET_NAME,
        &serde_json::to_string(auth)?,
    )?;
    let auth_file = get_auth_file(forall_home);
    std::fs::write(&auth_file, "stale")?;
    Ok(auth_file)
}

fn seed_secrets_backend_with_auth(
    mock_keyring: &MockKeyringStore,
    forall_home: &Path,
    auth: &AuthDotJson,
) -> anyhow::Result<()> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        forall_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::ForallAuth,
    );
    manager.set(
        &SecretScope::Global,
        &CODEX_AUTH_SECRET_NAME,
        &serde_json::to_string(auth)?,
    )?;
    Ok(())
}

fn assert_keyring_saved_auth_and_removed_fallback(
    mock_keyring: &MockKeyringStore,
    forall_home: &Path,
    expected: &AuthDotJson,
) -> anyhow::Result<()> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        forall_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::ForallAuth,
    );
    let saved_value = manager
        .get(&SecretScope::Global, &CODEX_AUTH_SECRET_NAME)?
        .context("encrypted auth entry should exist")?;
    let expected_serialized = serde_json::to_string(expected)?;
    assert_eq!(saved_value, expected_serialized);
    let old_key = compute_store_key(forall_home)?;
    assert!(
        mock_keyring.saved_value(&old_key).is_none(),
        "legacy keyring auth entry should not be used"
    );
    let secrets_key = compute_keyring_account(forall_home);
    assert!(
        mock_keyring.saved_value(&secrets_key).is_some(),
        "secrets backend should persist an encryption passphrase in the keyring"
    );
    assert!(encrypted_auth_file(forall_home).exists());
    let auth_file = get_auth_file(forall_home);
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring save"
    );
    Ok(())
}

fn encrypted_auth_file(forall_home: &Path) -> PathBuf {
    forall_home.join("secrets").join("forall_auth.age")
}

fn id_token_with_prefix(prefix: &str) -> IdTokenInfo {
    #[derive(Serialize)]
    struct Header {
        alg: &'static str,
        typ: &'static str,
    }

    let header = Header {
        alg: "none",
        typ: "JWT",
    };
    let payload = json!({
        "email": format!("{prefix}@example.com"),
        "https://forall.astrio.app/auth": {
            "chatgpt_account_id": format!("{prefix}-account"),
        },
    });
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header_b64 = encode(&serde_json::to_vec(&header).expect("serialize header"));
    let payload_b64 = encode(&serde_json::to_vec(&payload).expect("serialize payload"));
    let signature_b64 = encode(b"sig");
    let fake_jwt = format!("{header_b64}.{payload_b64}.{signature_b64}");

    crate::token_data::parse_chatgpt_jwt_claims(&fake_jwt).expect("fake JWT should parse")
}

fn auth_with_prefix(prefix: &str) -> AuthDotJson {
    AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some(format!("{prefix}-api-key")),
        openrouter_api_key: None,
        tokens: Some(TokenData {
            id_token: id_token_with_prefix(prefix),
            access_token: format!("{prefix}-access"),
            refresh_token: format!("{prefix}-refresh"),
            account_id: Some(format!("{prefix}-account-id")),
        }),
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    }
}

#[test]
fn secrets_keyring_auth_storage_load_returns_deserialized_auth() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let expected = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-test".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
        github_connection: None,
    };
    seed_secrets_backend_with_auth(&mock_keyring, forall_home.path(), &expected)?;

    let loaded = storage.load()?;
    assert_eq!(Some(expected), loaded);
    Ok(())
}

#[test]
fn keyring_auth_storage_compute_store_key_for_home_directory() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let path = forall_home.path().join(".forall");

    let key = compute_store_key(path.as_path())?;

    assert!(key.starts_with("cli|"));
    assert_eq!(key.len(), "cli|".len() + 16);
    assert_eq!(key, compute_store_key(path.as_path())?);
    Ok(())
}

#[test]
fn direct_keyring_auth_storage_saves_legacy_keyring_entry() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = DirectKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth_file = get_auth_file(forall_home.path());
    std::fs::write(&auth_file, "stale")?;
    let auth = auth_with_prefix("direct");

    storage.save(&auth)?;

    let legacy_key = compute_store_key(forall_home.path())?;
    let saved_value = mock_keyring
        .saved_value(&legacy_key)
        .context("direct keyring auth entry should exist")?;
    assert_eq!(saved_value, serde_json::to_string(&auth)?);
    assert!(!encrypted_auth_file(forall_home.path()).exists());
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring save"
    );
    assert_eq!(storage.load()?, Some(auth));
    Ok(())
}

#[test]
fn direct_keyring_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = DirectKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("direct-delete");
    storage.save(&auth)?;
    let auth_file = get_auth_file(forall_home.path());
    std::fs::write(&auth_file, "stale")?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "keyring auth should be removed");
    assert!(
        mock_keyring
            .saved_value(&compute_store_key(forall_home.path())?)
            .is_none(),
        "legacy keyring auth entry should be removed"
    );
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    assert!(!encrypted_auth_file(forall_home.path()).exists());
    Ok(())
}

#[test]
fn factory_uses_secrets_backend_only_when_requested() -> anyhow::Result<()> {
    let direct_home = tempdir()?;
    let direct_keyring = MockKeyringStore::default();
    let direct_storage = create_auth_storage_with_store(
        direct_home.path().to_path_buf(),
        AuthCredentialsStoreMode::Keyring,
        Arc::new(direct_keyring.clone()),
        AuthKeyringBackendKind::Direct,
    );
    let direct_auth = auth_with_prefix("factory-direct");
    direct_storage.save(&direct_auth)?;
    assert!(
        direct_keyring
            .saved_value(&compute_store_key(direct_home.path())?)
            .is_some()
    );
    assert!(!encrypted_auth_file(direct_home.path()).exists());

    let secrets_home = tempdir()?;
    let secrets_keyring = MockKeyringStore::default();
    let secrets_storage = create_auth_storage_with_store(
        secrets_home.path().to_path_buf(),
        AuthCredentialsStoreMode::Keyring,
        Arc::new(secrets_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let secrets_auth = auth_with_prefix("factory-secrets");
    secrets_storage.save(&secrets_auth)?;
    assert!(
        secrets_keyring
            .saved_value(&compute_keyring_account(secrets_home.path()))
            .is_some()
    );
    assert!(encrypted_auth_file(secrets_home.path()).exists());
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_save_persists_and_removes_fallback_file() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth_file = get_auth_file(forall_home.path());
    std::fs::write(&auth_file, "stale")?;
    let auth = AuthDotJson {
        provider_keys: Default::default(),
        auth_mode: Some(AuthMode::ApiKey),
        openai_api_key: Some("sk-test-key".to_string()),
        openrouter_api_key: None,
        tokens: None,
        last_refresh: Some(Utc::now()),
        bedrock_api_key: None,
        github_connection: None,
    };

    storage.save(&auth)?;

    assert_keyring_saved_auth_and_removed_fallback(&mock_keyring, forall_home.path(), &auth)?;
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        forall_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_delete_removes_legacy_direct_keyring_entry() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let direct_storage = DirectKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    direct_storage.save(&auth_with_prefix("legacy-direct"))?;
    let storage = SecretsKeyringAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        forall_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert_eq!(
        direct_storage.load()?,
        None,
        "legacy direct keyring auth should be removed"
    );
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    Ok(())
}

#[test]
fn auto_auth_storage_load_prefers_keyring_value() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let keyring_auth = auth_with_prefix("keyring");
    seed_secrets_backend_with_auth(&mock_keyring, forall_home.path(), &keyring_auth)?;

    let file_auth = auth_with_prefix("file");
    storage.file_storage.save(&file_auth)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(keyring_auth));
    Ok(())
}

#[test]
fn auto_auth_storage_load_uses_file_when_keyring_empty() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring),
        AuthKeyringBackendKind::Secrets,
    );

    let expected = auth_with_prefix("file-only");
    storage.file_storage.save(&expected)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(expected));
    Ok(())
}

#[test]
fn auto_auth_storage_load_falls_back_when_keyring_errors() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let key = compute_keyring_account(forall_home.path());

    let encrypted = auth_with_prefix("encrypted");
    seed_secrets_backend_with_auth(&mock_keyring, forall_home.path(), &encrypted)?;
    mock_keyring.set_error(&key, KeyringError::Invalid("error".into(), "load".into()));

    let expected = auth_with_prefix("fallback");
    storage.file_storage.save(&expected)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(expected));
    Ok(())
}

#[test]
fn auto_auth_storage_save_prefers_keyring() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let stale = auth_with_prefix("stale");
    storage.file_storage.save(&stale)?;

    let expected = auth_with_prefix("to-save");
    storage.save(&expected)?;

    assert_keyring_saved_auth_and_removed_fallback(&mock_keyring, forall_home.path(), &expected)?;
    Ok(())
}

#[test]
fn auto_auth_storage_save_falls_back_when_keyring_errors() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let key = compute_keyring_account(forall_home.path());
    mock_keyring.set_error(&key, KeyringError::Invalid("error".into(), "save".into()));

    let auth = auth_with_prefix("fallback");
    storage.save(&auth)?;

    let auth_file = get_auth_file(forall_home.path());
    assert!(
        auth_file.exists(),
        "fallback auth.json should be created when keyring save fails"
    );
    let saved = storage
        .file_storage
        .load()?
        .context("fallback auth should exist")?;
    assert_eq!(saved, auth);
    assert!(
        mock_keyring.saved_value(&key).is_none(),
        "keyring should not contain value when save fails"
    );
    Ok(())
}

#[test]
fn auto_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        forall_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        forall_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after delete"
    );
    Ok(())
}

/// Credentials are replaced by rename, never truncated in place.
///
/// The old `save` opened auth.json with `truncate(true)`, so any reader in the
/// window between emptying the file and finishing the write saw an empty or
/// partial file, failed to parse it, and signed the user out. A crash in that
/// window left it that way permanently.
#[tokio::test]
async fn saving_credentials_never_leaves_a_partial_file() -> anyhow::Result<()> {
    let forall_home = tempdir()?;
    let storage = FileAuthStorage::new(forall_home.path().to_path_buf());
    let auth_file = forall_home.path().join("auth.json");

    let first = AuthDotJson {
        openai_api_key: Some("sk-first".to_string()),
        ..Default::default()
    };
    storage.save(&first)?;

    // Overwrite with a payload of a different length: a truncating write would
    // pass through a state that is neither value.
    let second = AuthDotJson {
        openai_api_key: Some("sk-second-and-considerably-longer".to_string()),
        ..Default::default()
    };
    storage.save(&second)?;

    let raw = std::fs::read_to_string(&auth_file)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).context("auth.json must always parse")?;
    assert_eq!(
        parsed["OPENAI_API_KEY"], "sk-second-and-considerably-longer",
        "the newest write must win whole"
    );

    // No temp file may survive a successful save.
    let strays: Vec<_> = std::fs::read_dir(forall_home.path())?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.contains("tmp-"))
        .collect();
    assert!(strays.is_empty(), "left temp files behind: {strays:?}");

    Ok(())
}

/// A secret must never be readable by anyone else, including for the moment
/// between creating the replacement and renaming it into place.
#[cfg(unix)]
#[tokio::test]
async fn saved_credentials_are_owner_only() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let forall_home = tempdir()?;
    let storage = FileAuthStorage::new(forall_home.path().to_path_buf());
    storage.save(&AuthDotJson {
        openai_api_key: Some("sk-secret".to_string()),
        ..Default::default()
    })?;

    let mode = std::fs::metadata(forall_home.path().join("auth.json"))?
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "auth.json must stay owner read/write only");
    Ok(())
}
