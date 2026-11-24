use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Display;

use forall_config::types::DEFAULT_OTEL_API_KEY_ENV_VAR;
use forall_config::types::DEFAULT_OTEL_ENDPOINT_ENV_VAR;
use forall_config::types::DEFAULT_OTEL_ENVIRONMENT;
use forall_config::types::OtelConfig;
use forall_config::types::OtelConfigToml;
use forall_config::types::OtelExporterKind;
use forall_config::types::OtelHttpProtocol;

pub(crate) fn resolve_config(
    config: OtelConfigToml,
    startup_warnings: &mut Vec<String>,
) -> OtelConfig {
    let endpoint_base = resolve_endpoint_base(&config);
    let exporter_explicit = config.exporter.is_some();
    let trace_exporter_explicit = config.trace_exporter.is_some();
    let metrics_exporter_explicit = config.metrics_exporter.is_some();

    let log_user_prompt = config.log_user_prompt.unwrap_or(false);
    let environment = config
        .environment
        .unwrap_or_else(|| DEFAULT_OTEL_ENVIRONMENT.to_string());
    let api_key_env = config.api_key_env.clone();

    let mut exporter = config.exporter.unwrap_or(OtelExporterKind::None);
    let mut trace_exporter = config.trace_exporter.unwrap_or(OtelExporterKind::None);
    let (mut metrics_exporter, mut metrics_exporter_configured) = match config.metrics_exporter {
        Some(exporter) => (exporter, true),
        None => (OtelExporterKind::None, false),
    };

    if let Some(base) = endpoint_base {
        if !exporter_explicit {
            exporter = otlp_http_endpoint(&base, "/v1/logs");
        }
        if !trace_exporter_explicit {
            trace_exporter = otlp_http_endpoint(&base, "/v1/traces");
        }
        if !metrics_exporter_explicit {
            metrics_exporter = otlp_http_endpoint(&base, "/v1/metrics");
            metrics_exporter_configured = true;
        }
    }

    // Provider initialization installs process-global OTEL state. Sanitize
    // user-editable trace metadata here so malformed config is reported as a
    // startup warning instead of making startup fail.
    let span_attributes = resolve_span_attributes(config.span_attributes, startup_warnings);
    let tracestate = resolve_tracestate(config.tracestate, startup_warnings);

    OtelConfig {
        log_user_prompt,
        environment,
        exporter,
        trace_exporter,
        metrics_exporter,
        metrics_exporter_configured,
        api_key_env,
        span_attributes,
        tracestate,
    }
}

fn resolve_endpoint_base(config: &OtelConfigToml) -> Option<String> {
    config
        .endpoint
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::env::var(DEFAULT_OTEL_ENDPOINT_ENV_VAR)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
}

fn otlp_http_endpoint(base: &str, path: &str) -> OtelExporterKind {
    let base = base.trim().trim_end_matches('/');
    OtelExporterKind::OtlpHttp {
        endpoint: format!("{base}{path}"),
        headers: HashMap::new(),
        protocol: OtelHttpProtocol::Json,
        tls: None,
    }
}

pub(crate) fn otel_api_key_env_var(config: &OtelConfig) -> &str {
    config
        .api_key_env
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_OTEL_API_KEY_ENV_VAR)
}

fn resolve_span_attributes(
    span_attributes: Option<BTreeMap<String, String>>,
    startup_warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let Some(span_attributes) = span_attributes else {
        return BTreeMap::new();
    };

    let mut valid_attributes = BTreeMap::new();
    for (key, value) in span_attributes {
        let attribute = BTreeMap::from([(key.clone(), value.clone())]);
        if let Err(err) = forall_otel::validate_span_attributes(&attribute) {
            push_invalid_config_warning("otel.span_attributes", err, startup_warnings);
            continue;
        }
        valid_attributes.insert(key, value);
    }

    valid_attributes
}

fn resolve_tracestate(
    tracestate: Option<BTreeMap<String, BTreeMap<String, String>>>,
    startup_warnings: &mut Vec<String>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let Some(tracestate) = tracestate else {
        return BTreeMap::new();
    };

    let mut valid_entries = BTreeMap::new();
    for (member_key, fields) in tracestate {
        let fields = resolve_tracestate_member_fields(&member_key, fields, startup_warnings);
        if fields.is_empty() {
            continue;
        }
        if let Err(err) = forall_otel::validate_tracestate_member(&member_key, &fields) {
            push_invalid_config_warning("otel.tracestate", err, startup_warnings);
            continue;
        }
        valid_entries.insert(member_key, fields);
    }

    // Tracestate members can be valid individually while the combined W3C
    // tracestate header is not, so validate the filtered set before handing it
    // to provider initialization.
    if let Err(err) = forall_otel::validate_tracestate_entries(&valid_entries) {
        push_invalid_config_warning("otel.tracestate", err, startup_warnings);
        return BTreeMap::new();
    }

    valid_entries
}

fn resolve_tracestate_member_fields(
    member_key: &str,
    fields: BTreeMap<String, String>,
    startup_warnings: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let mut valid_fields = BTreeMap::new();
    for (field_key, value) in fields {
        let field = BTreeMap::from([(field_key.clone(), value.clone())]);
        if let Err(err) = forall_otel::validate_tracestate_member(member_key, &field) {
            push_invalid_config_warning("otel.tracestate", err, startup_warnings);
            continue;
        }
        valid_fields.insert(field_key, value);
    }
    valid_fields
}

fn push_invalid_config_warning(
    config_key: &str,
    err: impl Display,
    startup_warnings: &mut Vec<String>,
) {
    let message = format!("Ignoring invalid `{config_key}` config: {err}");
    tracing::warn!("{message}");
    startup_warnings.push(message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_base_fills_unset_otlp_exporters() {
        let config = resolve_config(
            OtelConfigToml {
                endpoint: Some("https://otel.forall.astrio.app".to_string()),
                ..Default::default()
            },
            &mut Vec::new(),
        );

        assert_eq!(
            config.trace_exporter,
            otlp_http_endpoint("https://otel.forall.astrio.app", "/v1/traces")
        );
        assert_eq!(
            config.metrics_exporter,
            otlp_http_endpoint("https://otel.forall.astrio.app", "/v1/metrics")
        );
        assert!(config.metrics_exporter_configured);
        assert_eq!(
            config.exporter,
            otlp_http_endpoint("https://otel.forall.astrio.app", "/v1/logs")
        );
    }

    #[test]
    fn explicit_exporters_are_not_overridden_by_endpoint() {
        let config = resolve_config(
            OtelConfigToml {
                endpoint: Some("https://otel.forall.astrio.app".to_string()),
                trace_exporter: Some(OtelExporterKind::None),
                ..Default::default()
            },
            &mut Vec::new(),
        );

        assert_eq!(config.trace_exporter, OtelExporterKind::None);
    }
}
