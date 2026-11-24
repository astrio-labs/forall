mod client;
mod config;
mod error;
pub(crate) mod names;
mod process;
pub(crate) mod runtime_metrics;
pub(crate) mod tags;
pub(crate) mod timer;
pub(crate) mod validation;

pub use crate::metrics::client::MetricsClient;
pub use crate::metrics::config::MetricsConfig;
pub use crate::metrics::error::MetricsError;
pub use crate::metrics::error::Result;
pub use crate::metrics::process::record_process_start_once;
pub use names::*;
pub use tags::ORIGINATOR_TAG;
pub use tags::SessionMetricTagValues;
pub use tags::bounded_originator_tag_value;

static LOCAL_RUNTIME_METRICS: std::sync::OnceLock<MetricsClient> = std::sync::OnceLock::new();

pub(crate) fn local_runtime_metrics() -> MetricsClient {
    LOCAL_RUNTIME_METRICS
        .get_or_init(MetricsClient::local_runtime)
        .clone()
}

pub fn global() -> Option<MetricsClient> {
    LOCAL_RUNTIME_METRICS.get().cloned()
}
