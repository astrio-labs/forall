mod catalog;

use std::path::PathBuf;
use std::sync::Arc;

use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_model_provider_info::ANTHROPIC_CLAUDE_OPUS_MODEL_ID;
use forall_model_provider_info::ModelProviderInfo;
use forall_models_manager::manager::SharedModelsManager;
use forall_models_manager::manager::StaticModelsManager;
use forall_protocol::openai_models::ModelsResponse;

use crate::auth::auth_manager_for_provider;
use crate::provider::ModelProvider;
use crate::provider::ModelProviderFuture;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;
use catalog::static_model_catalog;
use catalog::with_default_only_service_tier;

/// Exposes the bundled Claude catalog so the Claude-on-Bedrock catalog can be
/// asserted equal to it: same model, same Messages adapter, so the two metadata
/// claims must not drift.
#[cfg(test)]
pub(crate) fn static_model_catalog_for_test() -> ModelsResponse {
    static_model_catalog()
}

/// Runtime provider for the Anthropic Messages API.
///
/// Auth is handled by the shared BYOK resolution path: the provider's
/// `auth_header` (`x-api-key`) routes the stored `ANTHROPIC_API_KEY` through
/// [`crate::header_auth_provider::HeaderAuthProvider`], and the pinned
/// `anthropic-version` header ships in the provider's static `http_headers`.
/// Errors are likewise mapped in `forall-api`'s Messages endpoint before they
/// reach this layer, so no `map_api_error` override is needed.
#[derive(Clone, Debug)]
pub(crate) struct AnthropicModelProvider {
    info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl AnthropicModelProvider {
    pub(crate) fn new(
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        let auth_manager = auth_manager_for_provider(auth_manager, &provider_info);
        Self {
            info: provider_info,
            auth_manager,
        }
    }
}

impl ModelProvider for AnthropicModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            namespace_tools: false,
            image_generation: false,
            web_search: false,
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn memory_extraction_preferred_model(&self) -> &'static str {
        ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn memory_consolidation_preferred_model(&self) -> &'static str {
        ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<ForallAuth>> {
        Box::pin(async move {
            match self.auth_manager.as_ref() {
                Some(auth_manager) => auth_manager.auth().await,
                None => None,
            }
        })
    }

    fn account_state(&self) -> ProviderAccountResult {
        // BYOK provider: no first-party account to surface.
        Ok(ProviderAccountState {
            account: None,
            requires_openai_auth: false,
        })
    }

    fn models_manager(
        &self,
        _forall_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        Arc::new(StaticModelsManager::new(
            /*auth_manager*/ None,
            config_model_catalog.map_or_else(static_model_catalog, with_default_only_service_tier),
        ))
    }
}

#[cfg(test)]
mod tests {
    use forall_models_manager::ModelsManagerConfig;
    use forall_models_manager::manager::RefreshStrategy;
    use pretty_assertions::assert_eq;

    use super::*;

    fn anthropic_provider() -> AnthropicModelProvider {
        AnthropicModelProvider::new(
            ModelProviderInfo::create_anthropic_provider(),
            /*auth_manager*/ None,
        )
    }

    fn test_forall_home() -> PathBuf {
        std::env::temp_dir().join(format!(
            "forall-anthropic-provider-test-{}",
            std::process::id()
        ))
    }

    #[test]
    fn capabilities_disable_hosted_tools_and_namespace_tools() {
        assert_eq!(
            anthropic_provider().capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
    }

    #[test]
    fn preferred_models_use_flagship_claude_slug() {
        let provider = anthropic_provider();

        assert_eq!(
            provider.approval_review_preferred_model(),
            ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
    }

    #[test]
    fn account_state_reports_no_account() {
        assert_eq!(
            anthropic_provider().account_state(),
            Ok(ProviderAccountState {
                account: None,
                requires_openai_auth: false,
            })
        );
    }

    #[tokio::test]
    async fn models_manager_serves_bundled_claude_catalog() {
        let provider = anthropic_provider();
        let manager =
            provider.models_manager(test_forall_home(), /*config_model_catalog*/ None);

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;
        let model_ids = catalog
            .models
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            model_ids,
            vec!["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"]
        );

        let default_model = manager
            .list_models(RefreshStrategy::Online)
            .await
            .into_iter()
            .find(|preset| preset.is_default)
            .expect("Anthropic catalog should have a default model");
        assert_eq!(default_model.model, ANTHROPIC_CLAUDE_OPUS_MODEL_ID);
    }

    #[tokio::test]
    async fn get_model_info_resolves_bundled_metadata_without_fallback() {
        let provider = anthropic_provider();
        let manager =
            provider.models_manager(test_forall_home(), /*config_model_catalog*/ None);
        let config = ModelsManagerConfig::default();

        for slug in ["claude-opus-4-6", "claude-sonnet-4-6", "claude-haiku-4-5"] {
            let model_info = manager.get_model_info(slug, &config).await;

            assert_eq!(model_info.slug, slug);
            assert!(
                !model_info.used_fallback_model_metadata,
                "{slug} must resolve to bundled metadata, not the fallback"
            );
            assert_eq!(model_info.context_window, Some(200_000));
            assert_eq!(model_info.max_context_window, Some(200_000));
            assert!(model_info.supports_reasoning_summaries);
            assert!(model_info.supports_parallel_tool_calls);
        }
    }

    #[tokio::test]
    async fn configured_anthropic_catalog_only_allows_default_service_tier() {
        let configured_model = forall_models_manager::bundled_models_response()
            .expect("bundled models should parse")
            .models
            .into_iter()
            .find(|model| model.slug == "gpt-5.5")
            .expect("bundled models should include GPT-5.5");
        assert!(!configured_model.additional_speed_tiers.is_empty());
        assert!(!configured_model.service_tiers.is_empty());

        let provider = anthropic_provider();
        let manager = provider.models_manager(
            test_forall_home(),
            Some(ModelsResponse {
                models: vec![configured_model],
            }),
        );

        let catalog = manager.raw_model_catalog(RefreshStrategy::Online).await;

        assert_eq!(catalog.models.len(), 1);
        assert_eq!(
            catalog.models[0].additional_speed_tiers,
            Vec::<String>::new()
        );
        assert_eq!(catalog.models[0].service_tiers, Vec::new());
        assert_eq!(catalog.models[0].default_service_tier, None);
    }
}
