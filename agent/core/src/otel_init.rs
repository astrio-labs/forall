use crate::config::Config;
use crate::config::otel::otel_api_key_env_var;
use forall_config::types::OtelExporterKind as Kind;
use forall_config::types::OtelHttpProtocol as Protocol;
use forall_features::Feature;
use forall_login::default_client::originator;
use forall_otel::OtelExporter;
use forall_otel::OtelHttpProtocol;
use forall_otel::OtelProvider;
use forall_otel::OtelSettings;
use forall_otel::OtelTlsConfig as OtelTlsSettings;
use std::collections::HashMap;
use std::error::Error;

/// Build an OpenTelemetry provider from the app Config.
///
/// Returns `None` when OTEL export is disabled.
///
/// `default_metrics_enabled` controls whether OTEL metrics export is enabled when
/// `[otel].metrics_exporter` was not set explicitly in config. Product analytics
/// (`[analytics].enabled`) is independent of OTEL metrics.
pub fn build_provider(
    config: &Config,
    service_version: &str,
    service_name_override: Option<&str>,
    default_metrics_enabled: bool,
) -> Result<Option<OtelProvider>, Box<dyn Error>> {
    let to_otel_exporter = |kind: &Kind| match kind {
        Kind::None => OtelExporter::None,
        Kind::OtlpHttp {
            endpoint,
            headers,
            protocol,
            tls,
        } => {
            let protocol = match protocol {
                Protocol::Json => OtelHttpProtocol::Json,
                Protocol::Binary => OtelHttpProtocol::Binary,
            };

            OtelExporter::OtlpHttp {
                endpoint: endpoint.clone(),
                headers: headers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                protocol,
                tls: tls.as_ref().map(|config| OtelTlsSettings {
                    ca_certificate: config.ca_certificate.clone(),
                    client_certificate: config.client_certificate.clone(),
                    client_private_key: config.client_private_key.clone(),
                }),
            }
        }
        Kind::OtlpGrpc {
            endpoint,
            headers,
            tls,
        } => OtelExporter::OtlpGrpc {
            endpoint: endpoint.clone(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            tls: tls.as_ref().map(|config| OtelTlsSettings {
                ca_certificate: config.ca_certificate.clone(),
                client_certificate: config.client_certificate.clone(),
                client_private_key: config.client_private_key.clone(),
            }),
        },
    };

    let api_key_env = otel_api_key_env_var(&config.otel);
    let exporter = inject_otel_api_key(to_otel_exporter(&config.otel.exporter), api_key_env);
    let trace_exporter =
        inject_otel_api_key(to_otel_exporter(&config.otel.trace_exporter), api_key_env);
    let metrics_exporter = if config.otel.metrics_exporter_configured || default_metrics_enabled {
        inject_otel_api_key(to_otel_exporter(&config.otel.metrics_exporter), api_key_env)
    } else {
        OtelExporter::None
    };

    let originator = originator();
    let service_name = service_name_override.unwrap_or(originator.value.as_str());
    let runtime_metrics = config.features.enabled(Feature::RuntimeMetrics);

    OtelProvider::from(&OtelSettings {
        service_name: service_name.to_string(),
        service_version: service_version.to_string(),
        forall_home: config.forall_home.to_path_buf(),
        environment: config.otel.environment.to_string(),
        exporter,
        trace_exporter,
        metrics_exporter,
        runtime_metrics,
        span_attributes: config.otel.span_attributes.clone(),
        tracestate: config.otel.tracestate.clone(),
    })
}

fn inject_otel_api_key(exporter: OtelExporter, api_key_env: &str) -> OtelExporter {
    match exporter {
        OtelExporter::OtlpHttp {
            endpoint,
            mut headers,
            protocol,
            tls,
        } => {
            inject_bearer_header(&mut headers, api_key_env);
            OtelExporter::OtlpHttp {
                endpoint,
                headers,
                protocol,
                tls,
            }
        }
        OtelExporter::OtlpGrpc {
            endpoint,
            mut headers,
            tls,
        } => {
            inject_bearer_header(&mut headers, api_key_env);
            OtelExporter::OtlpGrpc {
                endpoint,
                headers,
                tls,
            }
        }
        other => other,
    }
}

fn inject_bearer_header(headers: &mut HashMap<String, String>, api_key_env: &str) {
    if headers.contains_key("Authorization") {
        return;
    }
    let Ok(key) = std::env::var(api_key_env) else {
        return;
    };
    let key = key.trim();
    if key.is_empty() {
        return;
    }
    headers.insert("Authorization".to_string(), format!("Bearer {key}"));
}

/// Filter predicate for exporting only Forall-owned events via OTEL.
/// Keeps events that originated from forall_otel module
pub fn forall_export_filter(meta: &tracing::Metadata<'_>) -> bool {
    meta.target().starts_with("forall_otel")
}

pub fn record_process_start(otel: Option<&OtelProvider>, originator: &str) {
    let Some(metrics) = otel.and_then(OtelProvider::metrics) else {
        return;
    };
    let _ = forall_otel::record_process_start_once(metrics, originator);
}

pub fn install_sqlite_telemetry(otel: Option<&OtelProvider>, originator: &str) {
    let Some(metrics) = otel.and_then(OtelProvider::metrics) else {
        return;
    };
    let telemetry = forall_rollout::sqlite_telemetry_recorder(metrics.clone(), originator);
    let _ = forall_state::install_process_db_telemetry(telemetry);
}

#[cfg(test)]
mod tests {
    use super::build_provider;
    use crate::config::ConfigBuilder;
    use forall_config::types::OtelExporterKind;
    use forall_config::types::OtelHttpProtocol;
    use std::collections::HashMap;
    use tempfile::TempDir;

    const SERVICE_VERSION: &str = "0.0.0-test";

    fn otlp_metrics_config(config: &mut crate::config::Config) {
        config.otel.metrics_exporter = OtelExporterKind::OtlpHttp {
            endpoint: "http://localhost:4318".to_string(),
            headers: HashMap::new(),
            protocol: OtelHttpProtocol::Json,
            tls: None,
        };
        config.otel.metrics_exporter_configured = true;
    }

    #[tokio::test]
    async fn metrics_respect_surface_default_when_exporter_unconfigured() {
        let forall_home = TempDir::new().expect("tempdir");
        let config = ConfigBuilder::default()
            .forall_home(forall_home.path().to_path_buf())
            .build()
            .await
            .expect("config");

        let disabled = build_provider(
            &config,
            SERVICE_VERSION,
            Some("forall-app-server"),
            /*default_metrics_enabled*/ false,
        )
        .expect("provider");
        assert!(disabled.as_ref().and_then(|otel| otel.metrics()).is_none());
    }

    #[tokio::test]
    async fn explicit_metrics_exporter_enables_metrics_without_surface_default() {
        let forall_home = TempDir::new().expect("tempdir");
        let mut config = ConfigBuilder::default()
            .forall_home(forall_home.path().to_path_buf())
            .build()
            .await
            .expect("config");
        otlp_metrics_config(&mut config);

        let provider = build_provider(
            &config,
            SERVICE_VERSION,
            Some("forall-app-server"),
            /*default_metrics_enabled*/ false,
        )
        .expect("provider");
        assert!(provider.as_ref().and_then(|otel| otel.metrics()).is_some());
    }

    #[tokio::test]
    async fn metrics_ignore_product_analytics_enabled_flag() {
        let forall_home = TempDir::new().expect("tempdir");
        let mut config = ConfigBuilder::default()
            .forall_home(forall_home.path().to_path_buf())
            .build()
            .await
            .expect("config");
        otlp_metrics_config(&mut config);
        config.analytics_enabled = Some(false);

        let provider = build_provider(
            &config,
            SERVICE_VERSION,
            Some("forall-app-server"),
            /*default_metrics_enabled*/ false,
        )
        .expect("provider");
        assert!(provider.as_ref().and_then(|otel| otel.metrics()).is_some());
    }
}
