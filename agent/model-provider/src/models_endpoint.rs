use std::sync::Arc;
use std::time::Duration;

use forall_api::ModelsClient;
use forall_api::RequestTelemetry;
use forall_api::ReqwestTransport;
use forall_api::TransportError;
use forall_api::auth_header_telemetry;
use forall_api::map_api_error;
use forall_feedback::FeedbackRequestTags;
use forall_feedback::emit_feedback_request_tags_with_auth_env;
use forall_login::AuthEnvTelemetry;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_login::collect_auth_env_telemetry;
use forall_login::default_client::build_reqwest_client;
use forall_model_provider_info::ModelProviderInfo;
use forall_models_manager::manager::ModelsEndpointClient;
use forall_models_manager::manager::ModelsEndpointFuture;
use forall_otel::TelemetryAuthMode;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result as CoreResult;
use forall_protocol::openai_models::ModelInfo;
use forall_response_debug_context::extract_response_debug_context;
use forall_response_debug_context::telemetry_transport_error_message;
use http::HeaderMap;
use tokio::time::timeout;

use crate::auth::resolve_provider_auth_with_manager;

const MODELS_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
const MODELS_ENDPOINT: &str = "/models";

/// Provider-owned OpenAI-compatible `/models` endpoint.
#[derive(Debug)]
pub(crate) struct OpenAiModelsEndpoint {
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl OpenAiModelsEndpoint {
    pub(crate) fn new(
        provider_info: ModelProviderInfo,
        auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        Self {
            provider_info,
            auth_manager,
        }
    }

    async fn auth(&self) -> Option<ForallAuth> {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager.auth().await,
            None => None,
        }
    }

    async fn uses_forall_backend(&self) -> bool {
        self.auth()
            .await
            .as_ref()
            .is_some_and(ForallAuth::uses_forall_backend)
    }

    async fn list_models(
        &self,
        client_version: &str,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        let _timer =
            forall_otel::start_global_timer("forall.remote_models.fetch_update.duration_ms", &[]);
        let auth = self.auth().await;
        let auth_mode = auth.as_ref().map(ForallAuth::auth_mode);
        let api_provider = self.provider_info.to_api_provider(auth_mode)?;
        let api_auth = resolve_provider_auth_with_manager(
            auth.as_ref(),
            &self.provider_info,
            self.auth_manager.as_deref(),
        )?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let auth_telemetry = auth_header_telemetry(api_auth.as_ref());
        let request_telemetry: Arc<dyn RequestTelemetry> = Arc::new(ModelsRequestTelemetry {
            auth_mode: auth_mode.map(|mode| TelemetryAuthMode::from(mode).to_string()),
            auth_header_attached: auth_telemetry.attached,
            auth_header_name: auth_telemetry.name,
            auth_env: self.auth_env(),
        });
        let client = ModelsClient::new(transport, api_provider, api_auth)
            .with_telemetry(Some(request_telemetry));

        let (body, etag) = timeout(
            MODELS_REFRESH_TIMEOUT,
            client.fetch_models_body(client_version, HeaderMap::new()),
        )
        .await
        .map_err(|_| ForallErr::Timeout)?
        .map_err(map_api_error)?;

        forall_models_manager::openai_compatible_catalog::parse_model_catalog_response(&body)
            .map(|models| (models, etag))
            .map_err(|err| {
                ForallErr::InvalidRequest(format!(
                    "failed to decode models response: {err}; body: {}",
                    String::from_utf8_lossy(&body)
                ))
            })
    }

    fn auth_env(&self) -> AuthEnvTelemetry {
        let forall_api_key_env_enabled = self
            .auth_manager
            .as_ref()
            .is_some_and(|auth_manager| auth_manager.forall_api_key_env_enabled());
        collect_auth_env_telemetry(&self.provider_info, forall_api_key_env_enabled)
    }
}

impl ModelsEndpointClient for OpenAiModelsEndpoint {
    fn has_command_auth(&self) -> bool {
        self.provider_info.has_command_auth()
    }

    fn uses_provider_model_catalog(&self) -> bool {
        self.provider_info.uses_provider_model_catalog()
    }

    fn uses_forall_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(OpenAiModelsEndpoint::uses_forall_backend(self))
    }

    fn list_models<'a>(
        &'a self,
        client_version: &'a str,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(OpenAiModelsEndpoint::list_models(self, client_version))
    }
}

#[derive(Clone)]
struct ModelsRequestTelemetry {
    auth_mode: Option<String>,
    auth_header_attached: bool,
    auth_header_name: Option<&'static str>,
    auth_env: AuthEnvTelemetry,
}

impl RequestTelemetry for ModelsRequestTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<http::StatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let success = status.is_some_and(|code| code.is_success()) && error.is_none();
        let error_message = error.map(telemetry_transport_error_message);
        let response_debug = error
            .map(extract_response_debug_context)
            .unwrap_or_default();
        let status = status.map(|status| status.as_u16());
        tracing::event!(
            target: "forall_otel.log_only",
            tracing::Level::INFO,
            event.name = "forall.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.env_openai_api_key_present = self.auth_env.openai_api_key_env_present,
            auth.env_forall_api_key_present = self.auth_env.forall_api_key_env_present,
            auth.env_forall_api_key_enabled = self.auth_env.forall_api_key_env_enabled,
            auth.env_provider_key_name = self.auth_env.provider_env_key_name.as_deref(),
            auth.env_provider_key_present = self.auth_env.provider_env_key_present,
            auth.env_refresh_token_url_override_present = self.auth_env.refresh_token_url_override_present,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
        tracing::event!(
            target: "forall_otel.trace_safe",
            tracing::Level::INFO,
            event.name = "forall.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.env_openai_api_key_present = self.auth_env.openai_api_key_env_present,
            auth.env_forall_api_key_present = self.auth_env.forall_api_key_env_present,
            auth.env_forall_api_key_enabled = self.auth_env.forall_api_key_env_enabled,
            auth.env_provider_key_name = self.auth_env.provider_env_key_name.as_deref(),
            auth.env_provider_key_present = self.auth_env.provider_env_key_present,
            auth.env_refresh_token_url_override_present = self.auth_env.refresh_token_url_override_present,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
        emit_feedback_request_tags_with_auth_env(
            &FeedbackRequestTags {
                endpoint: MODELS_ENDPOINT,
                auth_header_attached: self.auth_header_attached,
                auth_header_name: self.auth_header_name,
                auth_mode: self.auth_mode.as_deref(),
                auth_retry_after_unauthorized: None,
                auth_recovery_mode: None,
                auth_recovery_phase: None,
                auth_connection_reused: None,
                auth_request_id: response_debug.request_id.as_deref(),
                auth_cf_ray: response_debug.cf_ray.as_deref(),
                auth_error: response_debug.auth_error.as_deref(),
                auth_error_code: response_debug.auth_error_code.as_deref(),
                auth_recovery_followup_success: None,
                auth_recovery_followup_status: None,
            },
            &self.auth_env,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use super::*;
    use forall_protocol::config_types::ModelProviderAuthInfo;

    fn provider_info_with_command_auth() -> ModelProviderInfo {
        ModelProviderInfo {
            auth: Some(ModelProviderAuthInfo {
                command: "print-token".to_string(),
                args: Vec::new(),
                timeout_ms: NonZeroU64::new(5_000).expect("timeout should be non-zero"),
                refresh_interval_ms: 300_000,
                cwd: std::env::current_dir()
                    .expect("current dir should be available")
                    .try_into()
                    .expect("current dir should be absolute"),
            }),
            requires_openai_auth: false,
            ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
        }
    }

    #[test]
    fn command_auth_provider_reports_command_auth_without_cached_auth() {
        let endpoint = OpenAiModelsEndpoint::new(
            provider_info_with_command_auth(),
            /*auth_manager*/ None,
        );

        assert!(endpoint.has_command_auth());
    }

    #[test]
    fn provider_without_command_auth_reports_no_command_auth() {
        let endpoint = OpenAiModelsEndpoint::new(
            ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert!(!endpoint.has_command_auth());
    }
}
