use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::mapping::schema::{PhaseStatus, VerificationSummary, VerifyReport};
use crate::paths::{verify_reports_dir, verify_reports_read_dir};
use crate::verify::check_inputs::write_check_inputs_fingerprint;

pub fn format_verification_tier(summary: &VerificationSummary) -> String {
    let VerificationSummary {
        proved_requirements,
        property_tested_requirements,
        spec_tracked_requirements,
        total_requirements,
    } = summary;
    if *total_requirements == 0 {
        return "no mapped requirements in scope".to_string();
    }
    let mut parts = Vec::new();
    parts.push(format!("{proved_requirements} proved"));
    if *property_tested_requirements > 0 {
        parts.push(format!("{property_tested_requirements} property-tested"));
    }
    // Deliberately not "spec-tracked". This count is a remainder — every
    // requirement that earned neither a proof nor a property run — so it also
    // holds `contracted` ones, where a machine-checkable contract exists and
    // simply was not discharged. Calling those spec-tracked understates them
    // and contradicts the ledger the desktop reads from.
    if *spec_tracked_requirements > 0 {
        parts.push(format!("{spec_tracked_requirements} not machine-checked"));
    }
    if *proved_requirements == 0 && *property_tested_requirements == 0 {
        return format!("0 proved, {spec_tracked_requirements} not machine-checked");
    }
    parts.join(", ")
}

pub fn verify_report_json_path(root: &Path, change: Option<&str>) -> std::path::PathBuf {
    let stem = change.unwrap_or("project");
    verify_reports_read_dir(root).join(format!("{stem}-verify.json"))
}

pub fn read_verify_report(root: &Path, change: Option<&str>) -> Option<VerifyReport> {
    let json_path = verify_report_json_path(root, change);
    if !json_path.is_file() {
        return None;
    }
    let raw = fs::read_to_string(json_path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn print_report(report: &VerifyReport) {
    let critical = report
        .issues
        .iter()
        .filter(|i| i.severity == crate::mapping::schema::IssueSeverity::Critical)
        .count();
    let warnings = report
        .issues
        .iter()
        .filter(|i| i.severity == crate::mapping::schema::IssueSeverity::Warning)
        .count();

    println!("forall check — {}", if report.ok { "PASS" } else { "FAIL" });
    println!("project: {}", report.root);
    if let Some(change) = &report.change {
        println!("change:  {change}");
    }
    println!();
    println!("Phases:");
    for (phase, status) in &report.phases {
        let status_str = match status {
            PhaseStatus::Pass => "pass",
            PhaseStatus::Fail => "fail",
            PhaseStatus::Skip => "skip",
        };
        println!("  {phase}: {status_str}");
    }

    if !report.issues.is_empty() {
        println!();
        for issue in &report.issues {
            let prefix = match issue.severity {
                crate::mapping::schema::IssueSeverity::Critical => "✗",
                crate::mapping::schema::IssueSeverity::Warning => "!",
                crate::mapping::schema::IssueSeverity::Info => "·",
            };
            let req = issue
                .requirement_id
                .as_ref()
                .map(|id| format!("[{id}] "))
                .unwrap_or_default();
            let file = issue
                .file
                .as_ref()
                .map(|f| format!(" ({})", crate::diagnostic::terminal_safe(f)))
                .unwrap_or_default();
            // Continuation lines are indented rather than escaped. A message
            // legitimately carries multi-line prover output, so flattening it
            // would cost the diagnostic its shape — but a mapping-supplied
            // path reaches these messages too, and a newline in one would
            // otherwise put attacker-chosen text flush left, where it reads as
            // a line Forall printed itself.
            println!(
                "{prefix} {} {}: {req}{}{}",
                issue.severity.as_str(),
                issue.phase,
                issue.message.replace('\n', "\n    "),
                file
            );
            if let Some(ce) = &issue.counterexample {
                println!("    counterexample: {ce}");
            }
            if let Some(pbt) = &issue.pbt {
                println!("    pbt: seed={}, examples={}", pbt.seed, pbt.examples_run);
            }
        }
    }

    println!();
    println!(
        "Verification: {}",
        format_verification_tier(&report.verification_summary)
    );
    println!();
    println!(
        "Summary: {critical} critical, {warnings} warning(s), {} verified file(s)",
        report.verified_files.len()
    );
}

pub fn write_report_artifacts(root: &Path, report: &VerifyReport) -> std::io::Result<()> {
    let reports_dir = verify_reports_dir(root);
    fs::create_dir_all(&reports_dir)?;
    let stem = report.change.as_deref().unwrap_or("project");
    let json_path = reports_dir.join(format!("{stem}-verify.json"));
    fs::write(
        &json_path,
        format!("{}\n", serde_json::to_string_pretty(report)?),
    )?;
    if report.ok {
        let _ =
            write_check_inputs_fingerprint(root, report.change.as_deref(), &report.verified_files);
    }

    let md_path = reports_dir.join(format!("{stem}-verify.md"));
    let mut lines = vec![
        "# Forall verify report".to_string(),
        String::new(),
        format!("- Result: **{}**", if report.ok { "PASS" } else { "FAIL" }),
        format!("- Project: `{}`", report.root),
    ];
    if let Some(change) = &report.change {
        lines.push(format!("- Change: `{change}`"));
    }
    let vs = &report.verification_summary;
    lines.push(format!(
        "- Verification: {} proved, {} property-tested, {} not machine-checked ({} total)",
        vs.proved_requirements,
        vs.property_tested_requirements,
        vs.spec_tracked_requirements,
        vs.total_requirements
    ));
    lines.push(String::new());
    lines.push("## Phases".to_string());
    lines.push(String::new());
    for (phase, status) in &report.phases {
        let status_str = match status {
            PhaseStatus::Pass => "pass",
            PhaseStatus::Fail => "fail",
            PhaseStatus::Skip => "skip",
        };
        lines.push(format!("- {phase}: {status_str}"));
    }
    if !report.issues.is_empty() {
        lines.push(String::new());
        lines.push("## Issues".to_string());
        lines.push(String::new());
        for issue in &report.issues {
            let req = issue
                .requirement_id
                .as_ref()
                .map(|id| format!("`{id}` — "))
                .unwrap_or_default();
            lines.push(format!(
                "- **{}** ({}) {}{}",
                issue.severity.as_str(),
                issue.phase,
                req,
                issue.message
            ));
        }
    }
    fs::write(md_path, format!("{}\n", lines.join("\n")))?;
    Ok(())
}

pub fn exit_code_for_report(report: &VerifyReport) -> i32 {
    let has_critical = report
        .issues
        .iter()
        .any(|i| i.severity == crate::mapping::schema::IssueSeverity::Critical);
    if has_critical {
        return 1;
    }
    let has_warning = report
        .issues
        .iter()
        .any(|i| i.severity == crate::mapping::schema::IssueSeverity::Warning);
    if has_warning && report.strict {
        return 2;
    }
    if report.ok { 0 } else { 1 }
}

pub fn default_phases() -> BTreeMap<String, PhaseStatus> {
    let mut phases = BTreeMap::new();
    phases.insert("structure".to_string(), PhaseStatus::Pass);
    phases.insert("mapping".to_string(), PhaseStatus::Pass);
    phases.insert("proofs".to_string(), PhaseStatus::Pass);
    phases.insert("intent".to_string(), PhaseStatus::Skip);
    phases.insert("scenarios".to_string(), PhaseStatus::Skip);
    phases.insert("property-tests".to_string(), PhaseStatus::Skip);
    phases.insert("scenario-tests".to_string(), PhaseStatus::Skip);
    phases
}
