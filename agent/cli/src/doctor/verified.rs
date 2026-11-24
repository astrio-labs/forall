use std::path::Path;

use forall_workflow::doctor::{WorkflowDoctorStatus, run_verified_doctor};

use super::CheckStatus;
use super::DoctorCheck;

pub(super) fn verified_checks(cwd: &Path) -> Vec<DoctorCheck> {
    run_verified_doctor(cwd)
        .into_iter()
        .map(to_doctor_check)
        .collect()
}

fn to_doctor_check(check: forall_workflow::doctor::WorkflowDoctorCheck) -> DoctorCheck {
    let status = match check.status {
        WorkflowDoctorStatus::Ok => CheckStatus::Ok,
        WorkflowDoctorStatus::Warning => CheckStatus::Warning,
        WorkflowDoctorStatus::Fail => CheckStatus::Fail,
    };
    let category = verified_display_category(&check.id);
    let mut row =
        DoctorCheck::new(check.id, category, status, check.summary).details(check.details);
    if let Some(remediation) = check.remediation {
        row = row.remediation(remediation);
    }
    row
}

fn verified_display_category(id: &str) -> &'static str {
    match id {
        "verified.project" => "project",
        "verified.trust" => "trust",
        "verified.hooks" => "hooks",
        "verified.cli" => "cli",
        "verified.proof_engine" => "proof",
        "verified.adapters" => "adapters",
        "verified.active_change" => "change",
        id if id.starts_with("verified.proof_engine.") => "proof",
        _ => "verified",
    }
}
