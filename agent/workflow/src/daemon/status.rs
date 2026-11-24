use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::mapping::schema::{IssueSeverity, VerificationSummary, VerifyIssue, VerifyReport};
use crate::paths::{FORALL_DIR, ensure_dir};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonTopIssue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    pub phase: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatusSnapshot {
    pub updated_at: String,
    pub active_change: Option<String>,
    pub checking: bool,
    pub last_check_ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_summary: Option<VerificationSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_phases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_issue: Option<DaemonTopIssue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub fn daemon_status_path(root: &Path) -> std::path::PathBuf {
    root.join(FORALL_DIR)
        .join("verified")
        .join("daemon-status.json")
}

pub fn read_daemon_status(root: &Path) -> Option<DaemonStatusSnapshot> {
    let path = daemon_status_path(root);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_daemon_status(root: &Path, status: &DaemonStatusSnapshot) -> std::io::Result<()> {
    let dir = root.join(FORALL_DIR).join("verified");
    ensure_dir(&dir)?;
    let path = daemon_status_path(root);
    let tmp = dir.join(".daemon-status.json.tmp");
    fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(status)?))?;
    fs::rename(tmp, path)
}

pub fn checking_snapshot(active_change: Option<&str>) -> DaemonStatusSnapshot {
    DaemonStatusSnapshot {
        updated_at: chrono_now(),
        active_change: active_change.map(str::to_string),
        checking: true,
        last_check_ok: None,
        verification_summary: None,
        failed_phases: Vec::new(),
        top_issue: None,
        last_error: None,
    }
}

pub fn error_snapshot(active_change: Option<&str>, message: String) -> DaemonStatusSnapshot {
    DaemonStatusSnapshot {
        updated_at: chrono_now(),
        active_change: active_change.map(str::to_string),
        checking: false,
        last_check_ok: None,
        verification_summary: None,
        failed_phases: Vec::new(),
        top_issue: None,
        last_error: Some(message),
    }
}

pub fn snapshot_from_report(
    active_change: Option<&str>,
    report: &VerifyReport,
) -> DaemonStatusSnapshot {
    let failed_phases: Vec<String> = report
        .phases
        .iter()
        .filter(|(_, status)| **status == crate::mapping::schema::PhaseStatus::Fail)
        .map(|(name, _)| name.clone())
        .collect();
    let top_issue = top_issue_from_report(&report.issues).map(|issue| DaemonTopIssue {
        requirement_id: issue.requirement_id.clone(),
        phase: issue.phase.clone(),
        message: issue.message.clone(),
        counterexample: issue.counterexample.clone(),
    });
    DaemonStatusSnapshot {
        updated_at: chrono_now(),
        active_change: active_change.map(str::to_string).or(report.change.clone()),
        checking: false,
        last_check_ok: Some(report.ok),
        verification_summary: Some(report.verification_summary.clone()),
        failed_phases,
        top_issue,
        last_error: None,
    }
}

fn top_issue_from_report(issues: &[VerifyIssue]) -> Option<&VerifyIssue> {
    issues
        .iter()
        .find(|i| i.severity == IssueSeverity::Critical)
        .or_else(|| issues.first())
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}
