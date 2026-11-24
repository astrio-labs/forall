use forall_api::ImageEditRequest;
use forall_api::ImageGenerationRequest;
use forall_api::ImageResponse;
use forall_api::ImagesClient;
use forall_api::ReqwestTransport;
use forall_login::default_client::build_reqwest_client;
use forall_model_provider::SharedModelProvider;
use http::HeaderMap;

#[derive(Clone)]
pub(crate) struct ForallImagesBackend {
    provider: SharedModelProvider,
}

impl ForallImagesBackend {
    /// Creates a backend that sends image requests through the active model provider.
    pub(crate) fn new(provider: SharedModelProvider) -> Self {
        Self { provider }
    }

    /// Resolves the provider and auth required for the current image API request.
    async fn client(&self) -> Result<ImagesClient<ReqwestTransport>, String> {
        let provider = self
            .provider
            .api_provider()
            .await
            .map_err(|err| err.to_string())?;
        let auth = self
            .provider
            .api_auth()
            .await
            .map_err(|err| err.to_string())?;
        Ok(ImagesClient::new(
            ReqwestTransport::new(build_reqwest_client()),
            provider,
            auth,
        ))
    }

    /// Sends a standalone image generation request through the configured Images client.
    pub(crate) async fn generate(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageResponse, String> {
        self.client()
            .await?
            .generate(&request, HeaderMap::new())
            .await
            .map_err(|err| err.to_string())
    }

    /// Sends a standalone image edit request through the configured Images client.
    pub(crate) async fn edit(&self, request: ImageEditRequest) -> Result<ImageResponse, String> {
        self.client()
            .await?
            .edit(&request, HeaderMap::new())
            .await
            .map_err(|err| err.to_string())
    }
}
