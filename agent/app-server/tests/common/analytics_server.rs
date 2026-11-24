use anyhow::Result;
use forall_analytics::DEFAULT_ANALYTICS_EVENTS_PATH;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

pub async fn start_analytics_events_server() -> Result<MockServer> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DEFAULT_ANALYTICS_EVENTS_PATH))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Ok(server)
}

pub fn analytics_events_http_path() -> &'static str {
    DEFAULT_ANALYTICS_EVENTS_PATH
}
