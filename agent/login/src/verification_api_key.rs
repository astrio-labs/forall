use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use forall_secrets::SecretName;
use forall_secrets::SecretScope;
use forall_secrets::SecretsBackendKind;
use forall_secrets::SecretsManager;

pub const FORALL_VERIFICATION_API_KEY_ENV_VAR: &str = "FORALL_API_KEY";
const MAX_API_KEY_BYTES: usize = 4096;

#[derive(Clone)]
pub struct VerificationApiKeyStore {
    secrets: SecretsManager,
}

impl VerificationApiKeyStore {
    pub fn new(forall_home: &Path) -> Self {
        Self {
            secrets: SecretsManager::new(forall_home.to_path_buf(), SecretsBackendKind::Local),
        }
    }

    pub fn with_secrets_manager(secrets: SecretsManager) -> Self {
        Self { secrets }
    }

    pub fn set(&self, api_key: &str) -> Result<()> {
        let api_key = api_key.trim();
        anyhow::ensure!(
            !api_key.is_empty(),
            "verification API key must not be empty"
        );
        anyhow::ensure!(
            api_key.len() <= MAX_API_KEY_BYTES,
            "verification API key exceeds {MAX_API_KEY_BYTES} bytes"
        );
        self.secrets
            .set(&SecretScope::Global, &secret_name()?, api_key)
            .context("failed to store verification API key")
    }

    pub fn get(&self) -> Result<Option<String>> {
        if let Ok(value) = std::env::var(FORALL_VERIFICATION_API_KEY_ENV_VAR) {
            let value = value.trim();
            if !value.is_empty() {
                anyhow::ensure!(
                    value.len() <= MAX_API_KEY_BYTES,
                    "verification API key exceeds {MAX_API_KEY_BYTES} bytes"
                );
                return Ok(Some(value.to_string()));
            }
        }
        self.get_stored()
    }

    fn get_stored(&self) -> Result<Option<String>> {
        self.secrets
            .get(&SecretScope::Global, &secret_name()?)
            .context("failed to load verification API key")
            .map(|value| value.filter(|key| !key.trim().is_empty()))
    }

    pub fn is_configured(&self) -> bool {
        self.get().ok().flatten().is_some()
    }

    pub fn delete(&self) -> Result<bool> {
        self.secrets
            .delete(&SecretScope::Global, &secret_name()?)
            .context("failed to delete verification API key")
    }
}

fn secret_name() -> Result<SecretName> {
    SecretName::new(FORALL_VERIFICATION_API_KEY_ENV_VAR)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use forall_keyring_store::tests::MockKeyringStore;

    use super::*;

    fn store() -> VerificationApiKeyStore {
        let home = tempfile::tempdir().expect("tempdir");
        let manager = SecretsManager::new_with_keyring_store(
            home.path().to_path_buf(),
            SecretsBackendKind::Local,
            Arc::new(MockKeyringStore::default()),
        );
        VerificationApiKeyStore::with_secrets_manager(manager)
    }

    #[test]
    fn round_trips_without_exposing_the_value() {
        let store = store();
        store.set("forall_test_secret").expect("set");
        assert!(store.is_configured());
        assert_eq!(
            store.get_stored().expect("get").as_deref(),
            Some("forall_test_secret")
        );
        assert!(store.delete().expect("delete"));
        assert!(store.get_stored().expect("get after delete").is_none());
    }

    #[test]
    fn rejects_empty_and_oversized_keys() {
        let store = store();
        assert!(store.set("  ").is_err());
        assert!(store.set(&"x".repeat(MAX_API_KEY_BYTES + 1)).is_err());
    }
}
