mod accepted_lines;
#[cfg(debug_assertions)]
mod analytics_capture;
mod client;
mod client_config;
mod events;
mod facts;
mod ingest_schema;
mod reducer;

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

pub use accepted_lines::accepted_line_fingerprints_from_unified_diff;
pub use accepted_lines::fingerprint_hash;
pub use client::AnalyticsEventsClient;
pub use client_config::ANALYTICS_ENDPOINT_ENV_VAR;
pub use client_config::AnalyticsEventsClientConfig;
pub use client_config::DEFAULT_ANALYTICS_API_KEY_ENV_VAR;
pub use client_config::DEFAULT_ANALYTICS_EVENTS_PATH;
pub use client_config::normalize_analytics_events_url;
pub use client_config::resolve_telemetry_api_key;
pub use events::AppServerRpcTransport;
pub use events::GuardianApprovalRequestSource;
pub use events::GuardianReviewAnalyticsResult;
pub use events::GuardianReviewDecision;
pub use events::GuardianReviewEventParams;
pub use events::GuardianReviewFailureReason;
pub use events::GuardianReviewSessionAnalyticsParams;
pub use events::GuardianReviewSessionKind;
pub use events::GuardianReviewTerminalStatus;
pub use events::GuardianReviewTrackContext;
pub use events::GuardianReviewedAction;
pub use facts::AcceptedLineFingerprint;
pub use facts::AnalyticsJsonRpcError;
pub use facts::AppInvocation;
pub use facts::CompactionImplementation;
pub use facts::CompactionPhase;
pub use facts::CompactionReason;
pub use facts::CompactionStatus;
pub use facts::CompactionStrategy;
pub use facts::CompactionTrigger;
pub use facts::ExternalAgentConfigImportCompletedInput;
pub use facts::ExternalAgentConfigImportFailureInput;
pub use facts::ForallCheckCompletedFact;
pub use facts::ForallCheckCompletedInput;
pub use facts::ForallCompactionEvent;
pub use facts::ForallErrKind;
pub use facts::ForallGoalEvent;
pub use facts::ForallTurnSteerEvent;
pub use facts::GoalEventKind;
pub use facts::HookRunFact;
pub use facts::InputError;
pub use facts::InvocationType;
pub use facts::SkillInvocation;
pub use facts::SubAgentThreadStartedInput;
pub use facts::ThreadInitializationMode;
pub use facts::TrackEventsContext;
pub use facts::TurnForallErrorFact;
pub use facts::TurnProfile;
pub use facts::TurnProfileFact;
pub use facts::TurnResolvedConfigFact;
pub use facts::TurnStatus;
pub use facts::TurnSteerRejectionReason;
pub use facts::TurnSteerRequestError;
pub use facts::TurnSteerResult;
pub use facts::TurnTokenUsageFact;
pub use facts::VerifiedGateBlockedFact;
pub use facts::VerifiedGateBlockedInput;
pub use facts::build_track_events_context;
pub use ingest_schema::validate_ingest_payload;

#[cfg(test)]
mod analytics_client_tests;

pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn now_unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

pub(crate) fn serialize_enum_as_string<T: serde::Serialize>(value: &T) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
}

pub(crate) fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

pub(crate) fn option_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}
