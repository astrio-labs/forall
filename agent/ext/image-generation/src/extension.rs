use std::sync::Arc;

use forall_core::config::Config;
use forall_extension_api::ConfigContributor;
use forall_extension_api::ExtensionData;
use forall_extension_api::ExtensionFuture;
use forall_extension_api::ExtensionRegistryBuilder;
use forall_extension_api::ThreadLifecycleContributor;
use forall_extension_api::ThreadStartInput;
use forall_extension_api::ToolCall;
use forall_extension_api::ToolContributor;
use forall_extension_api::ToolExecutor;
use forall_login::AuthManager;
use forall_model_provider::create_model_provider;
use forall_model_provider_info::ModelProviderInfo;
use forall_utils_absolute_path::AbsolutePathBuf;

use crate::backend::ForallImagesBackend;
use crate::tool::ImageGenerationTool;

#[derive(Clone)]
struct ImageGenerationExtension {
    auth_manager: Arc<AuthManager>,
}

#[derive(Clone)]
struct ImageGenerationExtensionConfig {
    available: bool,
    provider: ModelProviderInfo,
    forall_home: AbsolutePathBuf,
}

impl From<&Config> for ImageGenerationExtensionConfig {
    /// Resolves whether standalone image generation should be available for a thread.
    fn from(config: &Config) -> Self {
        Self {
            // Core selects this executor per turn using the feature flag or model metadata.
            available: config.model_provider.is_openai(),
            provider: config.model_provider.clone(),
            forall_home: config.forall_home.clone(),
        }
    }
}

impl ThreadLifecycleContributor<Config> for ImageGenerationExtension {
    /// Seeds image-generation availability when a thread begins.
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            input
                .thread_store
                .insert(ImageGenerationExtensionConfig::from(input.config));
        })
    }
}

impl ConfigContributor<Config> for ImageGenerationExtension {
    /// Refreshes image-generation availability after thread configuration changes.
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        thread_store.insert(ImageGenerationExtensionConfig::from(new_config));
    }
}

impl ToolContributor for ImageGenerationExtension {
    /// Creates the image-generation tool exposed by this installed extension.
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(config) = thread_store.get::<ImageGenerationExtensionConfig>() else {
            return Vec::new();
        };
        if !config.available || !self.auth_manager.current_auth_uses_forall_backend() {
            return Vec::new();
        }

        vec![Arc::new(ImageGenerationTool::new(
            ForallImagesBackend::new(create_model_provider(
                config.provider.clone(),
                Some(self.auth_manager.clone()),
            )),
            config.forall_home.clone(),
            thread_store.level_id().to_string(),
        ))]
    }
}

/// Installs the standalone image-generation extension contributors.
pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let extension = Arc::new(ImageGenerationExtension { auth_manager });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}
