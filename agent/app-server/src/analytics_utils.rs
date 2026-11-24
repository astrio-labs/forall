use std::sync::Arc;

use forall_analytics::AnalyticsEventsClient;
use forall_core::config::Config;
use forall_login::AuthManager;

pub(crate) fn analytics_events_client_from_config(
    auth_manager: Arc<AuthManager>,
    config: &Config,
) -> AnalyticsEventsClient {
    AnalyticsEventsClient::new(auth_manager, config.analytics_events_client_config())
}
