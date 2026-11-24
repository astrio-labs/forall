use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

const DEFAULT_LOG_FILTER: &str = "error,opentelemetry_sdk=off,opentelemetry_otlp=off";

pub(crate) fn init() {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(stderr_env_filter());
    let _ = tracing_subscriber::registry().with(fmt_layer).try_init();
    tracing::callsite::rebuild_interest_cache();
}

fn stderr_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(DEFAULT_LOG_FILTER))
        .unwrap_or_else(|_| EnvFilter::new("error"))
}
