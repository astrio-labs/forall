use forall_analytics::AnalyticsEventsClient;
use forall_analytics::ForallCheckCompletedFact;
use forall_analytics::ForallCheckCompletedInput;
use forall_analytics::VerifiedGateBlockedFact;
use forall_analytics::VerifiedGateBlockedInput;
use forall_analytics::build_track_events_context;
use forall_workflow::mapping::schema::{IssueSeverity, PhaseStatus, VerifyReport};
use serde::Deserialize;

use crate::session::turn_context::TurnContext;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ParsedGateBlock {
    code: String,
    paths: Option<Vec<String>>,
    active_change: Option<String>,
    blocked_by: Option<Vec<String>>,
    next_artifact: Option<String>,
}

pub(crate) fn track_verified_gate_blocked(
    client: &AnalyticsEventsClient,
    turn: &TurnContext,
    thread_id: &str,
    tool_name: &str,
    block_payload: &str,
) {
    let Ok(gate) = serde_json::from_str::<ParsedGateBlock>(block_payload) else {
        tracing::debug!("skipping verified_gate_blocked analytics: invalid gate payload");
        return;
    };
    client.track_verified_gate_blocked(VerifiedGateBlockedInput {
        tracking: build_track_events_context(
            turn.model_info.slug.clone(),
            thread_id.to_string(),
            turn.sub_id.clone(),
        ),
        gate: VerifiedGateBlockedFact {
            tool: tool_name.to_string(),
            code: gate.code,
            paths: gate.paths,
            active_change: gate.active_change,
            blocked_by: gate.blocked_by,
            next_artifact: gate.next_artifact,
        },
    });
}

pub(crate) fn track_forall_check_completed(
    client: &AnalyticsEventsClient,
    turn: &TurnContext,
    thread_id: &str,
    report: &VerifyReport,
    duration_ms: u64,
    source: &str,
) {
    let (phases_pass, phases_fail, phases_skip) = phase_counts(&report.phases);
    let (critical_issues, warning_issues) = issue_counts(&report.issues);
    client.track_forall_check_completed(ForallCheckCompletedInput {
        tracking: build_track_events_context(
            turn.model_info.slug.clone(),
            thread_id.to_string(),
            turn.sub_id.clone(),
        ),
        check: ForallCheckCompletedFact {
            change: report.change.clone(),
            ok: report.ok,
            strict: report.strict,
            duration_ms,
            phases_pass,
            phases_fail,
            phases_skip,
            critical_issues,
            warning_issues,
            total_requirements: u32::try_from(report.verification_summary.total_requirements)
                .unwrap_or(u32::MAX),
            source: source.to_string(),
        },
    });
}

fn phase_counts(phases: &std::collections::BTreeMap<String, PhaseStatus>) -> (u32, u32, u32) {
    let mut pass = 0u32;
    let mut fail = 0u32;
    let mut skip = 0u32;
    for status in phases.values() {
        match status {
            PhaseStatus::Pass => pass = pass.saturating_add(1),
            PhaseStatus::Fail => fail = fail.saturating_add(1),
            PhaseStatus::Skip => skip = skip.saturating_add(1),
        }
    }
    (pass, fail, skip)
}

fn issue_counts(issues: &[forall_workflow::mapping::schema::VerifyIssue]) -> (u32, u32) {
    let mut critical = 0u32;
    let mut warning = 0u32;
    for issue in issues {
        match issue.severity {
            IssueSeverity::Critical => critical = critical.saturating_add(1),
            IssueSeverity::Warning => warning = warning.saturating_add(1),
            IssueSeverity::Info => {}
        }
    }
    (critical, warning)
}
