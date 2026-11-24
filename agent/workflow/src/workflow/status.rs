use std::path::Path;

use serde::Serialize;

use crate::active_change::resolve_active_change;
use crate::daemon::{DaemonStatusSnapshot, read_daemon_status};
use crate::mapping::schema::{VerificationSummary, VerifyReport};
use crate::paths::find_project_root;
use crate::verify::report::{read_verify_report, verify_report_json_path};
use crate::workflow::paths::list_active_changes;
use crate::workflow::schema::load_workflow_schema;
use crate::workflow::state::{compute_change_status, next_ready_artifact};
use crate::workflow::types::{ArtifactStatus, ChangeStatus};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStatusSnapshot {
    pub project_root: String,
    pub has_forall_project: bool,
    pub active_changes: Vec<String>,
    pub change_selection_required: bool,
    pub active_change: Option<String>,
    pub apply_ready: bool,
    pub apply_blocked_by: Vec<String>,
    pub next_artifact: Option<String>,
    pub change_class: String,
    pub status_hint: Option<String>,
    pub artifacts: Vec<ArtifactSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks_progress: Option<crate::workflow::types::TasksProgress>,
    pub last_check: Option<LastCheckSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daemon_status: Option<DaemonStatusSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactSnapshot {
    pub id: String,
    pub status: &'static str,
    pub generates: String,
    #[serde(rename = "blockedBy")]
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastCheckSnapshot {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    pub report_path: String,
    pub failed_phases: Vec<String>,
    pub verification_summary: VerificationSummary,
    pub top_issue: Option<TopIssueSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopIssueSnapshot {
    pub severity: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    pub message: String,
}

fn artifact_status_str(status: ArtifactStatus) -> &'static str {
    match status {
        ArtifactStatus::Done => "done",
        ArtifactStatus::Ready => "ready",
        ArtifactStatus::Blocked => "blocked",
    }
}

fn map_last_check(
    root: &Path,
    change: Option<&str>,
    report: Option<&VerifyReport>,
) -> Option<LastCheckSnapshot> {
    let report = report?;
    let failed_phases: Vec<String> = report
        .phases
        .iter()
        .filter(|(_, s)| **s == crate::mapping::schema::PhaseStatus::Fail)
        .map(|(name, _)| name.clone())
        .collect();
    let top = report
        .issues
        .iter()
        .find(|i| i.severity == crate::mapping::schema::IssueSeverity::Critical)
        .or_else(|| report.issues.first());
    Some(LastCheckSnapshot {
        ok: report.ok,
        change: report.change.clone().or_else(|| change.map(str::to_string)),
        report_path: verify_report_json_path(root, report.change.as_deref().or(change))
            .display()
            .to_string(),
        failed_phases,
        verification_summary: report.verification_summary.clone(),
        top_issue: top.map(|t| TopIssueSnapshot {
            severity: t.severity.as_str().to_string(),
            phase: t.phase.clone(),
            requirement_id: t.requirement_id.clone(),
            message: t.message.clone(),
        }),
    })
}

fn build_status_hint(
    has_forall_project: bool,
    active_changes: &[String],
    active_change: Option<&str>,
    apply_ready: bool,
) -> Option<String> {
    if !has_forall_project {
        return Some(
            "No Forall project — run `forall ensure` before editing application code.".to_string(),
        );
    }
    if active_changes.is_empty() {
        return Some(
            "No active change — run `forall propose <name>` before implementation work."
                .to_string(),
        );
    }
    if active_changes.len() > 1 && active_change.is_none() {
        return Some(format!(
            "Multiple active changes ({}) — set FORALL_CHANGE or write .forall/.active-change before editing app code.",
            active_changes.join(", ")
        ));
    }
    if let Some(change) = active_change {
        if !apply_ready {
            return Some(format!(
                "Change '{change}' is not apply-ready — complete workflow artifacts first."
            ));
        }
        return Some(format!(
            "Change '{change}' is apply-ready — app-code edits allowed when gates pass."
        ));
    }
    None
}

/// (active change, apply ready, apply blocked by, next artifact, artifacts, tasks progress)
type ChangeStatusSnapshotParts = (
    Option<String>,
    bool,
    Vec<String>,
    Option<String>,
    Vec<ArtifactSnapshot>,
    Option<crate::workflow::types::TasksProgress>,
);

fn snapshot_from_change_status(
    change: Option<String>,
    status: Option<&ChangeStatus>,
) -> ChangeStatusSnapshotParts {
    let Some(status) = status else {
        return (change, false, Vec::new(), None, Vec::new(), None);
    };
    let artifacts = status
        .artifacts
        .iter()
        .map(|a| ArtifactSnapshot {
            id: a.id.clone(),
            status: artifact_status_str(a.status),
            generates: a.generates.clone(),
            blocked_by: a.blocked_by.clone(),
        })
        .collect();
    (
        Some(status.name.clone()),
        status.apply_ready,
        status.apply_blocked_by.clone(),
        next_ready_artifact(status),
        artifacts,
        status.tasks_progress.clone(),
    )
}

pub fn build_workflow_status(start_dir: &Path, change: Option<&str>) -> WorkflowStatusSnapshot {
    let resolved_start = start_dir
        .canonicalize()
        .unwrap_or_else(|_| start_dir.to_path_buf());
    let project_root_found = find_project_root(&resolved_start);
    let project_root = project_root_found
        .clone()
        .unwrap_or_else(|| resolved_start.clone());
    let has_forall_project = project_root_found.is_some();
    let active_changes = if has_forall_project {
        list_active_changes(&project_root)
    } else {
        Vec::new()
    };

    let mut resolved_change = change.map(str::to_string);
    if has_forall_project && resolved_change.is_none() {
        resolved_change = resolve_active_change(&project_root);
    }

    let change_status = if has_forall_project {
        resolved_change.as_ref().and_then(|c| {
            load_workflow_schema(&project_root)
                .ok()
                .map(|schema| compute_change_status(&project_root, &schema, c))
        })
    } else {
        None
    };

    let (active_change, apply_ready, apply_blocked_by, next_artifact, artifacts, tasks_progress) =
        snapshot_from_change_status(resolved_change.clone(), change_status.as_ref());

    let change_selection_required =
        has_forall_project && active_changes.len() > 1 && resolved_change.is_none();

    let report = if has_forall_project {
        read_verify_report(&project_root, resolved_change.as_deref())
    } else {
        None
    };

    let daemon_status = if has_forall_project {
        read_daemon_status(&project_root)
    } else {
        None
    };

    WorkflowStatusSnapshot {
        project_root: project_root.display().to_string(),
        has_forall_project,
        active_changes,
        change_selection_required,
        active_change: active_change.clone(),
        apply_ready,
        apply_blocked_by,
        next_artifact,
        change_class: change_status
            .as_ref()
            .map(|s| s.workflow_class.as_str().to_string())
            .unwrap_or_else(|| "standard".to_string()),
        status_hint: build_status_hint(
            has_forall_project,
            &list_active_changes(&project_root),
            active_change.as_deref(),
            apply_ready,
        ),
        artifacts,
        tasks_progress,
        last_check: if has_forall_project {
            map_last_check(&project_root, resolved_change.as_deref(), report.as_ref())
        } else {
            None
        },
        daemon_status,
    }
}
