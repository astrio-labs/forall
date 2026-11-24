use crate::mapping::schema::{VerificationSummary, VerifyReport};
use crate::verify::report::format_verification_tier;

pub fn build_stop_check_success_message(
    change: &str,
    report: &VerifyReport,
    tier: Option<&str>,
) -> Option<String> {
    let summary = &report.verification_summary;
    if summary.total_requirements == 0 {
        return Some(format!(
            "Forall check passed for change '{change}' (no requirements in mapping.delta.yaml yet)."
        ));
    }
    if summary.proved_requirements == 0 {
        return Some(format!(
            "Forall check passed for change '{change}' ({}). Add //@ contracts and Dafny proofs (`verified: true`) before claiming formal verification.",
            tier.unwrap_or("")
        ));
    }
    Some(format!(
        "Forall check passed for change '{change}' ({}).",
        tier.unwrap_or("")
    ))
}

pub fn stop_tier_line(summary: &VerificationSummary) -> String {
    format_verification_tier(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::{PhaseStatus, VerifyReport};
    use std::collections::BTreeMap;

    fn sample_report(proved: usize, spec: usize) -> VerifyReport {
        VerifyReport {
            ok: true,
            strict: false,
            root: "/tmp".to_string(),
            change: Some("add-clamp".to_string()),
            phases: BTreeMap::from([("mapping".to_string(), PhaseStatus::Pass)]),
            issues: vec![],
            verified_files: vec![],
            verification_summary: VerificationSummary {
                total_requirements: proved + spec,
                proved_requirements: proved,
                property_tested_requirements: 0,
                spec_tracked_requirements: spec,
            },
        }
    }

    #[test]
    fn m1_c_includes_zero_proved_tier_line() {
        let report = sample_report(0, 1);
        let tier = stop_tier_line(&report.verification_summary);
        assert!(tier.contains("0 proved"));
        // The remainder bucket also holds `contracted` requirements, where a
        // machine-checkable contract exists and was simply not discharged, so
        // it must not be reported as the spec-tracked rung specifically.
        assert!(tier.contains("not machine-checked"), "{tier}");
        assert!(!tier.contains("spec-tracked"), "{tier}");

        let msg = build_stop_check_success_message("add-clamp", &report, Some(&tier)).unwrap();
        assert!(msg.contains("0 proved"));
        assert!(msg.contains("formal verification"));
    }
}
