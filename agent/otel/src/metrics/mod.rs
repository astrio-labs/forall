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
pub use crate::metrics::config::MetricsExporter;
pub use crate::metrics::error::MetricsError;
pub use crate::metrics::error::Result;
pub use crate::metrics::process::record_process_start_once;
pub use names::*;
use std::sync::OnceLock;
pub use tags::ORIGINATOR_TAG;
pub use tags::SessionMetricTagValues;
pub use tags::bounded_originator_tag_value;

static GLOBAL_METRICS: OnceLock<MetricsClient> = OnceLock::new();

pub(crate) fn install_global(metrics: MetricsClient) {
    let _ = GLOBAL_METRICS.set(metrics);
}

pub fn global() -> Option<MetricsClient> {
    GLOBAL_METRICS.get().cloned()
}
