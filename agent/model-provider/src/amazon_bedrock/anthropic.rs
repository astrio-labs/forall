use std::path::PathBuf;
use std::sync::Arc;

use forall_api::ApiError;
use forall_api::Provider;
use forall_api::SharedAuthProvider;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_login::auth::BedrockApiKeyAuth;
use forall_model_provider_info::BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID;
use forall_model_provider_info::ModelProviderAwsAuthInfo;
use forall_model_provider_info::ModelProviderInfo;
use forall_models_manager::manager::SharedModelsManager;
use forall_models_manager::manager::StaticModelsManager;
use forall_protocol::account::AmazonBedrockCredentialSource;
use forall_protocol::account::ProviderAccount;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result;
use forall_protocol::openai_models::ModelsResponse;

use crate::provider::ModelProvider;
use crate::provider::ModelProviderFuture;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;

use super::anthropic_catalog::static_model_catalog;
use super::anthropic_catalog::with_default_only_service_tier;
use super::auth::resolve_provider_auth;
use super::aws::BedrockEndpoint;
use super::error;
use super::runtime::runtime_base_url;

/// Runtime provider for Claude models on Amazon Bedrock's native
/// `bedrock-runtime` invoke API.
///
/// Deliberately a separate provider from [`super::AmazonBedrockModelProvider`]
/// rather than a per-model branch inside it: the two share only credential
/// resolution. This one targets `bedrock-runtime` (SigV4 service `bedrock`) and
/// speaks the Anthropic Messages schema over AWS event-stream framing; that one
/// targets the Mantle front door (SigV4 service `bedrock-mantle`) and speaks the
/// Responses API. Keeping them apart means the shipped GPT-on-Bedrock path is
/// untouched — including its `merge_configured_model_providers` special case.
///
/// The three-tier credential resolution (Forall-managed Bedrock API key →
/// `AWS_BEARER_TOKEN_BEDROCK` → SigV4 through the AWS config chain) *is* shared,
/// via [`resolve_provider_auth`]; only the signing service name differs.
#[derive(Clone, Debug)]
pub(crate) struct BedrockAnthropicModelProvider {
    info: ModelProviderInfo,
    aws: ModelProviderAwsAuthInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl BedrockAnthropicModelProvider {
    pub(crate) fn new(
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        let aws = provider_info
            .aws
            .clone()
            .unwrap_or(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            });
        Self {
            info: provider_info,
            aws,
            auth_manager,
        }
    }

    /// Only a Bedrock-scoped credential is accepted here; an OpenAI API key or
    /// ChatGPT token in the shared auth manager must never be forwarded to AWS.
    fn managed_auth(&self) -> Option<BedrockApiKeyAuth> {
        self.auth_manager
            .as_ref()
            .and_then(|auth_manager| auth_manager.auth_cached())
            .and_then(|auth| match auth {
                ForallAuth::BedrockApiKey(auth) => Some(auth),
                ForallAuth::ApiKey(_) | ForallAuth::Oauth(_) => None,
            })
    }

    async fn auth(&self) -> Option<ForallAuth> {
        self.managed_auth().map(ForallAuth::BedrockApiKey)
    }

    async fn api_provider(&self) -> Result<Provider> {
        let managed_auth = self.managed_auth();
        let mut api_provider_info = self.info.clone();
        api_provider_info.base_url =
            Some(runtime_base_url(managed_auth.as_ref(), &self.aws).await?);
        api_provider_info.to_api_provider(/*auth_mode*/ None)
    }

    async fn runtime_base_url(&self) -> Result<Option<String>> {
        let managed_auth = self.managed_auth();
        Ok(Some(
            runtime_base_url(managed_auth.as_ref(), &self.aws).await?,
        ))
    }

    async fn api_auth(&self) -> Result<SharedAuthProvider> {
        let managed_auth = self.managed_auth();
        resolve_provider_auth(managed_auth.as_ref(), &self.aws, BedrockEndpoint::Runtime).await
    }
}

impl ModelProvider for BedrockAnthropicModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Matches the direct-Anthropic provider: the Messages wire has no hosted
        // image-generation or web-search tool, and tool namespacing is an
        // OpenAI-side affordance.
        ProviderCapabilities {
            namespace_tools: false,
            image_generation: false,
            web_search: false,
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn memory_extraction_preferred_model(&self) -> &'static str {
        BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn memory_consolidation_preferred_model(&self) -> &'static str {
        BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.managed_auth()
            .and_then(|_| self.auth_manager.as_ref().cloned())
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<ForallAuth>> {
        Box::pin(BedrockAnthropicModelProvider::auth(self))
    }

    fn account_state(&self) -> ProviderAccountResult {
        let credential_source = if self.managed_auth().is_some() {
            AmazonBedrockCredentialSource::ForallManaged
        } else {
            AmazonBedrockCredentialSource::AwsManaged
        };
        Ok(ProviderAccountState {
            account: Some(ProviderAccount::AmazonBedrock { credential_source }),
            requires_openai_auth: false,
        })
    }

    fn map_api_error(&self, error: ApiError) -> ForallErr {
        // Shared with the Mantle path: both surface AWS-shaped failures.
        error::map_api_error(error)
    }

    fn api_provider(&self) -> ModelProviderFuture<'_, Result<Provider>> {
        Box::pin(BedrockAnthropicModelProvider::api_provider(self))
    }

    fn runtime_base_url(&self) -> ModelProviderFuture<'_, Result<Option<String>>> {
        Box::pin(BedrockAnthropicModelProvider::runtime_base_url(self))
    }

    fn api_auth(&self) -> ModelProviderFuture<'_, Result<SharedAuthProvider>> {
        Box::pin(BedrockAnthropicModelProvider::api_auth(self))
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

    fn provider() -> BedrockAnthropicModelProvider {
        BedrockAnthropicModelProvider::new(
            ModelProviderInfo::create_bedrock_anthropic_provider(/*aws*/ None),
            /*auth_manager*/ None,
        )
    }

    fn test_forall_home() -> PathBuf {
        std::env::temp_dir().join(format!(
            "forall-bedrock-anthropic-provider-test-{}",
            std::process::id()
        ))
    }

    #[test]
    fn capabilities_match_the_direct_anthropic_provider() {
        assert_eq!(
            provider().capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
    }

    #[test]
    fn preferred_models_use_the_bedrock_claude_slug() {
        let provider = provider();

        assert_eq!(
            provider.approval_review_preferred_model(),
            BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
        assert_eq!(
            provider.memory_extraction_preferred_model(),
            BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
        assert_eq!(
            provider.memory_consolidation_preferred_model(),
            BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID
        );
    }

    #[test]
    fn account_state_reports_aws_managed_credentials_by_default() {
        assert_eq!(
            provider().account_state(),
            Ok(ProviderAccountState {
                account: Some(ProviderAccount::AmazonBedrock {
                    credential_source: AmazonBedrockCredentialSource::AwsManaged,
                }),
                requires_openai_auth: false,
            })
        );
    }

    #[test]
    fn openai_auth_is_not_forwarded_to_aws() {
        let provider = BedrockAnthropicModelProvider::new(
            ModelProviderInfo::create_bedrock_anthropic_provider(/*aws*/ None),
            Some(AuthManager::from_auth_for_testing(
                ForallAuth::from_api_key("openai-api-key"),
            )),
        );

        assert!(provider.auth_manager().is_none());
    }

    #[tokio::test]
    async fn managed_bedrock_key_selects_the_bedrock_runtime_endpoint() {
        let managed_auth = BedrockApiKeyAuth {
            api_key: "managed-bedrock-api-key".to_string(),
            region: "eu-central-1".to_string(),
        };
        let provider = BedrockAnthropicModelProvider::new(
            ModelProviderInfo::create_bedrock_anthropic_provider(Some(ModelProviderAwsAuthInfo {
                profile: Some("aws-profile-that-should-not-be-loaded".to_string()),
                region: Some("us-west-2".to_string()),
            })),
            Some(AuthManager::from_auth_for_testing(
                ForallAuth::BedrockApiKey(managed_auth.clone()),
            )),
        );

        assert_eq!(
            provider
                .runtime_base_url()
                .await
                .expect("managed Bedrock region should resolve"),
            // The Mantle path would have produced a `bedrock-mantle...api.aws`
            // URL with an `/openai/v1` suffix.
            Some("https://bedrock-runtime.eu-central-1.amazonaws.com".to_string())
        );
        assert_eq!(
            provider.auth().await,
            Some(ForallAuth::BedrockApiKey(managed_auth))
        );
    }

    #[tokio::test]
    async fn models_manager_serves_the_bundled_bedrock_claude_catalog() {
        let provider = provider();
        let manager =
            provider.models_manager(test_forall_home(), /*config_model_catalog*/ None);

        let default_model = manager
            .list_models(RefreshStrategy::Online)
            .await
            .into_iter()
            .find(|preset| preset.is_default)
            .expect("catalog should have a default model");

        assert_eq!(default_model.model, BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID);
    }

    #[tokio::test]
    async fn bundled_slugs_resolve_without_the_unknown_model_fallback() {
        let provider = provider();
        let manager =
            provider.models_manager(test_forall_home(), /*config_model_catalog*/ None);
        let config = ModelsManagerConfig::default();

        for slug in [
            "anthropic.claude-opus-4-6",
            "anthropic.claude-sonnet-4-6",
            "anthropic.claude-haiku-4-5",
            // Cross-region inference profiles.
            "us.anthropic.claude-opus-4-6",
            "eu.anthropic.claude-sonnet-4-6",
            "apac.anthropic.claude-haiku-4-5",
        ] {
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
    async fn fully_qualified_aws_model_ids_resolve_by_prefix() {
        // Real on-demand ids carry a release date and version suffix. Metadata
        // lookup is a longest-prefix match, so the bundled slug covers them.
        let provider = provider();
        let manager =
            provider.models_manager(test_forall_home(), /*config_model_catalog*/ None);
        let config = ModelsManagerConfig::default();

        for slug in [
            "anthropic.claude-opus-4-6-20260101-v1:0",
            "us.anthropic.claude-sonnet-4-6-20260101-v1:0",
        ] {
            let model_info = manager.get_model_info(slug, &config).await;

            assert_eq!(model_info.slug, slug);
            assert!(
                !model_info.used_fallback_model_metadata,
                "{slug} must resolve to bundled metadata, not the fallback"
            );
            assert_eq!(model_info.context_window, Some(200_000));
        }
    }
}
