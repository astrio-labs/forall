use anyhow::Result;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::analytics_events_http_path;
use app_test_support::write_api_key_auth;
use forall_config::types::AuthCredentialsStoreMode;
use forall_config::types::OtelExporterKind;
use forall_config::types::OtelHttpProtocol;
use forall_core::config::ConfigBuilder;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

const SERVICE_VERSION: &str = "0.0.0-test";

fn set_metrics_exporter(config: &mut forall_core::config::Config) {
    config.otel.metrics_exporter = OtelExporterKind::OtlpHttp {
        endpoint: "http://localhost:4318".to_string(),
        headers: HashMap::new(),
        protocol: OtelHttpProtocol::Json,
        tls: None,
    };
    config.otel.metrics_exporter_configured = true;
}

#[tokio::test]
async fn app_server_default_metrics_disabled_without_flag() -> Result<()> {
    let forall_home = TempDir::new()?;
    let config = ConfigBuilder::default()
        .forall_home(forall_home.path().to_path_buf())
        .build()
        .await?;

    let provider = forall_core::otel_init::build_provider(
        &config,
        SERVICE_VERSION,
        Some("forall-app-server"),
        /*default_metrics_enabled*/ false,
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    // Without an explicit `[otel].metrics_exporter` and surface default false, metrics are off.
    let has_metrics = provider.as_ref().and_then(|otel| otel.metrics()).is_some();
    assert_eq!(has_metrics, false);
    Ok(())
}

#[tokio::test]
async fn explicit_metrics_exporter_enables_metrics_without_surface_default() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut config = ConfigBuilder::default()
        .forall_home(forall_home.path().to_path_buf())
        .build()
        .await?;
    set_metrics_exporter(&mut config);

    let provider = forall_core::otel_init::build_provider(
        &config,
        SERVICE_VERSION,
        Some("forall-app-server"),
        /*default_metrics_enabled*/ false,
    )
    .map_err(|err| anyhow::anyhow!(err.to_string()))?;

    let has_metrics = provider.as_ref().and_then(|otel| otel.metrics()).is_some();
    assert_eq!(has_metrics, true);
    Ok(())
}

pub(crate) async fn mount_analytics_capture(server: &MockServer, forall_home: &Path) -> Result<()> {
    let analytics_path = analytics_events_http_path();
    Mock::given(method("POST"))
        .and(path(analytics_path))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;

    append_analytics_endpoint_to_config(
        forall_home,
        &format!("{}{}", server.uri(), analytics_path),
    )?;

    write_api_key_auth(
        forall_home,
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    Ok(())
}

fn append_analytics_endpoint_to_config(forall_home: &Path, endpoint: &str) -> std::io::Result<()> {
    let config_path = forall_home.join("config.toml");
    let mut contents = std::fs::read_to_string(&config_path).unwrap_or_default();
    if !contents.contains("[analytics]") {
        contents.push_str(&format!(
            "\n[analytics]\nenabled = true\nendpoint = \"{endpoint}\"\n"
        ));
        std::fs::write(config_path, contents)?;
    }
    Ok(())
}

pub(crate) async fn wait_for_analytics_payload(
    server: &MockServer,
    read_timeout: Duration,
) -> Result<Value> {
    let analytics_path = analytics_events_http_path();
    let body = timeout(read_timeout, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            if let Some(request) = requests
                .iter()
                .find(|request| request.method == "POST" && request.url.path() == analytics_path)
            {
                break request.body.clone();
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?;
    serde_json::from_slice(&body).map_err(|err| anyhow::anyhow!("invalid analytics payload: {err}"))
}

pub(crate) async fn wait_for_analytics_event(
    server: &MockServer,
    read_timeout: Duration,
    event_type: &str,
) -> Result<Value> {
    wait_for_matching_analytics_event(server, read_timeout, |event| {
        event["event_type"] == event_type
    })
    .await
}

pub(crate) async fn wait_for_goal_event(
    server: &MockServer,
    read_timeout: Duration,
    event_kind: &str,
    goal_status: &str,
) -> Result<Value> {
    wait_for_matching_analytics_event(server, read_timeout, |event| {
        event["event_type"] == "forall_goal_event"
            && event["event_params"]["event_kind"] == event_kind
            && event["event_params"]["goal_status"] == goal_status
    })
    .await
}

async fn wait_for_matching_analytics_event(
    server: &MockServer,
    read_timeout: Duration,
    matches: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let analytics_path = analytics_events_http_path();
    timeout(read_timeout, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            };
            for request in &requests {
                if request.method != "POST" || request.url.path() != analytics_path {
                    continue;
                }
                let payload: Value = serde_json::from_slice(&request.body)
                    .map_err(|err| anyhow::anyhow!("invalid analytics payload: {err}"))?;
                let Some(events) = payload["events"].as_array() else {
                    continue;
                };
                if let Some(event) = events.iter().find(|event| matches(event)) {
                    return Ok::<Value, anyhow::Error>(event.clone());
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await?
}

pub(crate) fn thread_initialized_event(payload: &Value) -> Result<&Value> {
    let events = payload["events"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("analytics payload missing events array"))?;
    events
        .iter()
        .find(|event| event["event_type"] == "forall_thread_initialized")
        .ok_or_else(|| anyhow::anyhow!("forall_thread_initialized event should be present"))
}

pub(crate) fn assert_basic_thread_initialized_event(
    event: &Value,
    thread_id: &str,
    session_id: &str,
    expected_model: &str,
    initialization_mode: &str,
    expected_thread_source: &str,
) {
    assert_eq!(event["event_params"]["thread_id"], thread_id);
    assert_eq!(event["event_params"]["session_id"], session_id);
    assert_eq!(
        event["event_params"]["app_server_client"]["product_client_id"],
        DEFAULT_CLIENT_NAME
    );
    assert_eq!(
        event["event_params"]["app_server_client"]["client_name"],
        DEFAULT_CLIENT_NAME
    );
    assert_eq!(
        event["event_params"]["app_server_client"]["rpc_transport"],
        "stdio"
    );
    assert_eq!(event["event_params"]["model"], expected_model);
    assert_eq!(event["event_params"]["ephemeral"], false);
    assert_eq!(
        event["event_params"]["thread_source"],
        expected_thread_source
    );
    assert_eq!(
        event["event_params"]["subagent_source"],
        serde_json::Value::Null
    );
    assert_eq!(
        event["event_params"]["parent_thread_id"],
        serde_json::Value::Null
    );
    assert_eq!(
        event["event_params"]["initialization_mode"],
        initialization_mode
    );
    assert!(event["event_params"]["created_at"].as_u64().is_some());
}
