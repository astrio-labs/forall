use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::mapping::schema::{IssueSeverity, VerifyIssue, VerifyReport};
use crate::paths::{ensure_dir, verify_waivers_dir, verify_waivers_read_dir};
use crate::verify::report::verify_report_json_path;

pub const WAIVERS_REL: &str = ".forall/verify/cache/waivers";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WaiverRecord {
    pub timestamp: String,
    pub change: String,
    pub check_report_path: String,
    pub critical_issue_ids: Vec<String>,
    pub user_acknowledgment: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default = "default_agent_must_not_claim")]
    pub agent_must_not_claim: Vec<String>,
}

fn default_agent_must_not_claim() -> Vec<String> {
    vec![
        "proved".to_string(),
        "property-tested".to_string(),
        "formally-verified".to_string(),
    ]
}

pub fn waivers_dir(root: &Path) -> PathBuf {
    verify_waivers_dir(root)
}

pub fn waivers_read_dir(root: &Path) -> PathBuf {
    verify_waivers_read_dir(root)
}

pub fn stable_issue_id(issue: &VerifyIssue) -> String {
    if let Some(req) = &issue.requirement_id {
        format!("{req}:{}", issue.phase)
    } else {
        format!("{}:{}", issue.phase, issue.message)
    }
}

pub fn critical_issues(report: &VerifyReport) -> Vec<&VerifyIssue> {
    report
        .issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Critical)
        .collect()
}

/// Proof and property-test failures cannot be waived per stop-and-archive contract.
pub fn is_waivable_critical(issue: &VerifyIssue) -> bool {
    if issue.severity != IssueSeverity::Critical {
        return false;
    }
    !matches!(issue.phase.as_str(), "proofs" | "property-tests")
}

pub fn waivable_critical_ids(report: &VerifyReport) -> Vec<String> {
    report
        .issues
        .iter()
        .filter(|issue| is_waivable_critical(issue))
        .map(stable_issue_id)
        .collect()
}

pub fn non_waivable_critical_issues(report: &VerifyReport) -> Vec<&VerifyIssue> {
    report
        .issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Critical && !is_waivable_critical(issue))
        .collect()
}

pub fn waiver_needed(report: &VerifyReport) -> bool {
    !waivable_critical_ids(report).is_empty()
}

pub fn write_waiver_record(
    root: &Path,
    change: &str,
    report: &VerifyReport,
    user_acknowledgment: &str,
) -> Result<PathBuf, String> {
    let acknowledgment = user_acknowledgment.trim();
    if acknowledgment.is_empty() {
        return Err("user acknowledgment must not be empty".to_string());
    }

    let critical_issue_ids = waivable_critical_ids(report);
    if critical_issue_ids.is_empty() {
        return Err("check has no waivable CRITICAL issues".to_string());
    }

    let timestamp = Utc::now().to_rfc3339();
    let check_report_path = verify_report_json_path(root, Some(change))
        .strip_prefix(root)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| format!(".forall/verified/reports/{change}-verify.json"));

    let record = WaiverRecord {
        timestamp: timestamp.clone(),
        change: change.to_string(),
        check_report_path,
        critical_issue_ids,
        user_acknowledgment: acknowledgment.to_string(),
        session_id: std::env::var("FORALL_SESSION")
            .ok()
            .filter(|value| !value.trim().is_empty()),
        agent_must_not_claim: default_agent_must_not_claim(),
    };

    let dir = waivers_dir(root);
    ensure_dir(&dir).map_err(|err| err.to_string())?;
    let file_name = format!("{}.json", timestamp.replace([':', '.'], "-"));
    let path = dir.join(file_name);
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?
        ),
    )
    .map_err(|err| err.to_string())?;
    Ok(path)
}

pub fn find_waiver_covering(
    root: &Path,
    change: &str,
    critical_ids: &[String],
) -> Option<WaiverRecord> {
    if critical_ids.is_empty() {
        return None;
    }
    let dir = waivers_read_dir(root);
    if !dir.is_dir() {
        return None;
    }

    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort_by(|a, b| b.cmp(a));

    for path in paths {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<WaiverRecord>(&raw) else {
            continue;
        };
        if record.change != change {
            continue;
        }
        if critical_ids
            .iter()
            .all(|id| record.critical_issue_ids.iter().any(|saved| saved == id))
        {
            return Some(record);
        }
    }
    None
}

pub fn waiver_allows_archive(
    root: &Path,
    change: &str,
    report: &VerifyReport,
    fresh_ack: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    if !non_waivable_critical_issues(report).is_empty() {
        return Err("proof or property-test CRITICAL issues cannot be waived".to_string());
    }

    let critical_ids = waivable_critical_ids(report);
    if critical_ids.is_empty() {
        return Err("verification failed without waivable CRITICAL issues".to_string());
    }

    if let Some(ack) = fresh_ack {
        let path = write_waiver_record(root, change, report, ack)?;
        return Ok(Some(path));
    }

    if find_waiver_covering(root, change, &critical_ids).is_some() {
        return Ok(None);
    }

    Err(format!(
        "waivable CRITICAL issue(s) remain — run `forall waive --change {change} --ack \"<user confirmation>\"` or pass `--ack` to archive"
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::mapping::schema::{PhaseStatus, VerificationSummary, VerifyReport};

    fn sample_report(phase: &str) -> VerifyReport {
        VerifyReport {
            ok: false,
            strict: false,
            root: "/tmp/project".to_string(),
            change: Some("demo".to_string()),
            phases: BTreeMap::from([(phase.to_string(), PhaseStatus::Fail)]),
            issues: vec![VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: phase.to_string(),
                requirement_id: Some("req-1".to_string()),
                message: "failed".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            }],
            verified_files: Vec::new(),
            verification_summary: VerificationSummary {
                total_requirements: 1,
                proved_requirements: 0,
                property_tested_requirements: 0,
                spec_tracked_requirements: 1,
            },
        }
    }

    #[test]
    fn proofs_critical_is_not_waivable() {
        let report = sample_report("proofs");
        assert!(!is_waivable_critical(&report.issues[0]));
        assert!(non_waivable_critical_issues(&report).len() == 1);
    }

    #[test]
    fn mapping_critical_is_waivable() {
        let report = sample_report("mapping");
        assert!(is_waivable_critical(&report.issues[0]));
        assert_eq!(waivable_critical_ids(&report), vec!["req-1:mapping"]);
    }
}
