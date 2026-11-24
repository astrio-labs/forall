//! Validates product-analytics payloads against the Forall ingest contract.
//!
//! See `docs/telemetry/product-analytics-migration.md` — ingest expects
//! `{ "events": [ { "event_type": "...", "event_params": { ... } }, ... ] }`.

#[cfg(any(debug_assertions, test))]
use crate::events::TrackEventsRequest;
use serde_json::Value;

/// Validate a serialized ingest payload (JSON object).
pub fn validate_ingest_payload(value: &Value) -> Result<(), String> {
    let events = value
        .get("events")
        .ok_or_else(|| "missing top-level `events` array".to_string())?;
    let Some(events) = events.as_array() else {
        return Err("`events` must be an array".to_string());
    };
    if events.is_empty() {
        return Err("`events` must not be empty".to_string());
    }
    for (index, event) in events.iter().enumerate() {
        validate_ingest_event(event, index)?;
    }
    Ok(())
}

/// Validate a capture-file line or wire batch before delivery.
#[cfg(any(debug_assertions, test))]
pub(crate) fn validate_track_events_request(payload: &TrackEventsRequest) -> Result<(), String> {
    let value = serde_json::to_value(payload)
        .map_err(|err| format!("failed to serialize track events request: {err}"))?;
    validate_ingest_payload(&value)
}

fn validate_ingest_event(event: &Value, index: usize) -> Result<(), String> {
    let prefix = format!("events[{index}]");
    let Some(event_type) = event.get("event_type") else {
        return Err(format!("{prefix}: missing `event_type`"));
    };
    if !event_type.is_string() {
        return Err(format!("{prefix}: `event_type` must be a string"));
    }
    let event_type = event_type.as_str().unwrap_or_default();
    if event_type.is_empty() {
        return Err(format!("{prefix}: `event_type` must not be empty"));
    }
    let Some(params) = event.get("event_params") else {
        return Err(format!("{prefix}: missing `event_params`"));
    };
    if !params.is_object() {
        return Err(format!("{prefix}: `event_params` must be an object"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ForallCheckCompletedEventRequest;
    use crate::events::ForallCheckCompletedMetadata;
    use crate::events::TrackEventRequest;
    use serde_json::json;

    #[test]
    fn accepts_valid_batch_shape() {
        validate_ingest_payload(&json!({
            "events": [{
                "event_type": "forall_turn_event",
                "event_params": { "thread_id": "t1" }
            }]
        }))
        .expect("valid batch");
    }

    #[test]
    fn rejects_missing_events() {
        let err = validate_ingest_payload(&json!({})).expect_err("missing events");
        assert!(err.contains("events"));
    }

    #[test]
    fn rejects_event_without_params() {
        let err = validate_ingest_payload(&json!({
            "events": [{ "event_type": "forall_hook_run" }]
        }))
        .expect_err("missing params");
        assert!(err.contains("event_params"));
    }

    #[test]
    fn track_events_request_round_trip_matches_ingest_contract() {
        let payload = TrackEventsRequest {
            events: vec![TrackEventRequest::ForallCheckCompleted(
                ForallCheckCompletedEventRequest {
                    event_type: "forall_check_completed",
                    event_params: ForallCheckCompletedMetadata {
                        thread_id: Some("thread-1".to_string()),
                        turn_id: Some("turn-1".to_string()),
                        model_slug: Some("gpt-test".to_string()),
                        change: Some("add-clamp".to_string()),
                        ok: true,
                        strict: false,
                        duration_ms: 1200,
                        phases_pass: 7,
                        phases_fail: 0,
                        phases_skip: 0,
                        critical_issues: 0,
                        warning_issues: 1,
                        total_requirements: 3,
                        source: "specs.check".to_string(),
                        completed_at: 1_710_000_000_000,
                    },
                },
            )],
        };
        validate_track_events_request(&payload)
            .expect("serialized payload matches ingest contract");
    }
}
