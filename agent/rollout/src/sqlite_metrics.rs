use std::sync::Arc;
use std::time::Duration;

use forall_otel::ORIGINATOR_TAG;
use forall_otel::bounded_originator_tag_value;
use forall_state::DbTelemetry;
use forall_state::DbTelemetryHandle;

struct OtelDbTelemetry {
    metrics: forall_otel::MetricsClient,
    originator: &'static str,
}

impl DbTelemetry for OtelDbTelemetry {
    fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]) {
        let tags = with_originator(tags, self.originator);
        let _ = self.metrics.counter(name, inc, &tags);
    }

    fn record_duration(&self, name: &str, duration: Duration, tags: &[(&str, &str)]) {
        let tags = with_originator(tags, self.originator);
        let _ = self.metrics.record_duration(name, duration, &tags);
    }
}

pub(crate) fn recorder(metrics: forall_otel::MetricsClient, originator: &str) -> DbTelemetryHandle {
    Arc::new(OtelDbTelemetry {
        metrics,
        originator: bounded_originator_tag_value(originator),
    })
}

fn with_originator<'a>(
    tags: &[(&'a str, &'a str)],
    originator: &'static str,
) -> Vec<(&'a str, &'a str)> {
    let mut tags = tags.to_vec();
    tags.push((ORIGINATOR_TAG, originator));
    tags
}
