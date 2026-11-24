/// Env var fallback when `[analytics].endpoint` is unset.
pub const ANALYTICS_ENDPOINT_ENV_VAR: &str = "FORALL_ANALYTICS_ENDPOINT";

/// Default env var read for the telemetry ingest API key.
pub const DEFAULT_ANALYTICS_API_KEY_ENV_VAR: &str = "FORALL_ANALYTICS_API_KEY";

/// Appended when `endpoint` is a host URL without a path.
pub const DEFAULT_ANALYTICS_EVENTS_PATH: &str = "/v1/analytics/events";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnalyticsEventsClientConfig {
    /// Product analytics ingest URL. Host-only values get `DEFAULT_ANALYTICS_EVENTS_PATH`.
    pub endpoint: Option<String>,
    /// When `Some(false)`, the client is fully disabled.
    pub analytics_enabled: Option<bool>,
    /// Env var name holding the ingest API key. Defaults to `FORALL_ANALYTICS_API_KEY`.
    pub api_key_env: Option<String>,
}

impl AnalyticsEventsClientConfig {
    pub fn api_key_env_var(&self) -> &str {
        self.api_key_env
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or(DEFAULT_ANALYTICS_API_KEY_ENV_VAR)
    }

    pub fn resolve_endpoint(&self) -> Option<String> {
        let raw = self
            .endpoint
            .as_ref()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var(ANALYTICS_ENDPOINT_ENV_VAR)
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })?;
        Some(normalize_analytics_events_url(&raw))
    }
}

/// Runtime API key for the configured ingest service.
pub fn resolve_telemetry_api_key(api_key_env: &str) -> Option<String> {
    std::env::var(api_key_env)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn normalize_analytics_events_url(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.ends_with("/events") {
        return trimmed.to_string();
    }
    if let Some(rest) = trimmed.split("://").nth(1)
        && rest.contains('/')
    {
        return trimmed.to_string();
    }
    format!("{trimmed}{DEFAULT_ANALYTICS_EVENTS_PATH}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_appends_default_path_for_host_only_url() {
        assert_eq!(
            normalize_analytics_events_url("https://telemetry.forall.dev"),
            "https://telemetry.forall.dev/v1/analytics/events"
        );
    }

    #[test]
    fn normalize_preserves_explicit_path() {
        assert_eq!(
            normalize_analytics_events_url("https://telemetry.forall.dev/custom/events"),
            "https://telemetry.forall.dev/custom/events"
        );
    }

    #[test]
    fn normalize_preserves_legacy_chatgpt_backend_path() {
        assert_eq!(
            normalize_analytics_events_url(
                "https://chatgpt.com/backend-api/codex/analytics-events/events"
            ),
            "https://chatgpt.com/backend-api/codex/analytics-events/events"
        );
    }
}
