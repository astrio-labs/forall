//! `forall assurance check`: the ratchet, as a CI gate.
//!
//! Fails only on regression below the committed baseline or on targeted
//! paths below their declared minimum — never on absolute levels a team
//! inherited. On improvement the baseline is rewritten so progress locks
//! in automatically; on regression it is left untouched, because a ratchet
//! that loosens itself is decoration.

use std::path::Path;

use crate::assurance::{
    AssuranceReport, Finding, Waiver, evaluate_with_waivers, load_baseline, load_config,
    load_waivers, save_baseline, save_waivers,
};
use crate::paths::find_project_root;
use crate::verify::ledger::read_ledger;

pub fn run_assurance_check(root_arg: Option<&Path>, change: Option<&str>) -> i32 {
    let cwd = root_arg
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall assurance check: project not found");
        return 1;
    };

    let config = match load_config(&root) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("forall assurance check: {err}");
            return 1;
        }
    };
    let mut baseline = match load_baseline(&root) {
        Ok(baseline) => baseline,
        Err(err) => {
            eprintln!("forall assurance check: {err}");
            return 1;
        }
    };
    let Some(ledger) = read_ledger(&root, change) else {
        eprintln!(
            "forall assurance check: no verify ledger found — run `forall check` first \
             (a missing or tampered ledger reads as no evidence)"
        );
        return 1;
    };

    let waivers = match load_waivers(&root) {
        Ok(waivers) => waivers,
        Err(err) => {
            eprintln!("forall assurance check: {err}");
            return 1;
        }
    };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    for expired in waivers.expired(&today) {
        eprintln!(
            "EXPIRED WAIVER {}: {} (expired {})",
            expired.requirement_id, expired.reason, expired.expires
        );
    }
    let report = evaluate_with_waivers(&config, &mut baseline, &ledger, &waivers, &today);
    print_report(&report);

    let has_regression = report
        .findings
        .iter()
        .any(|finding| matches!(finding, Finding::Regression { .. }));
    if !has_regression {
        // Below-target findings still fail the gate but never block the
        // ratchet: improvement recorded is improvement kept.
        if let Err(err) = save_baseline(&root, &baseline) {
            eprintln!("forall assurance check: failed to write baseline: {err}");
            return 1;
        }
    }

    if report.findings.is_empty() { 0 } else { 1 }
}

fn print_report(report: &AssuranceReport) {
    for finding in &report.findings {
        match finding {
            Finding::Regression {
                requirement_id,
                baseline,
                earned,
            } => {
                eprintln!(
                    "REGRESSION {requirement_id}: earned {} below committed baseline {}",
                    earned.as_str(),
                    baseline.as_str()
                );
            }
            Finding::BelowTarget {
                requirement_id,
                file,
                target,
                earned,
            } => {
                eprintln!(
                    "BELOW TARGET {requirement_id} ({file}): earned {} — target {}",
                    earned.as_str(),
                    target.as_str()
                );
            }
        }
    }
    for finding in &report.waived {
        let id = match finding {
            Finding::Regression { requirement_id, .. }
            | Finding::BelowTarget { requirement_id, .. } => requirement_id,
        };
        println!("waived: {id} (unexpired waiver on file)");
    }
    if !report.ratcheted.is_empty() {
        println!(
            "assurance: baseline raised for {} requirement(s)",
            report.ratcheted.len()
        );
    }
    println!(
        "assurance: {} finding(s), {} exempt, {} untargeted",
        report.findings.len(),
        report.exempt,
        report.untargeted
    );
}

/// `forall assurance waive`: record a time-boxed exception. The expiry is
/// mandatory by design — see the module docs.
pub fn run_assurance_waive(
    root_arg: Option<&Path>,
    requirement_id: &str,
    expires_in_days: u32,
    reason: &str,
) -> i32 {
    let cwd = root_arg
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall assurance waive: project not found");
        return 1;
    };
    if reason.trim().is_empty() {
        eprintln!("forall assurance waive: a reason is required");
        return 1;
    }
    let mut waivers = match crate::assurance::load_waivers(&root) {
        Ok(waivers) => waivers,
        Err(err) => {
            eprintln!("forall assurance waive: {err}");
            return 1;
        }
    };
    let expires = (chrono::Utc::now() + chrono::Duration::days(i64::from(expires_in_days)))
        .format("%Y-%m-%d")
        .to_string();
    waivers
        .waivers
        .retain(|waiver| waiver.requirement_id != requirement_id);
    waivers.waivers.push(Waiver {
        requirement_id: requirement_id.to_string(),
        expires: expires.clone(),
        reason: reason.trim().to_string(),
    });
    if let Err(err) = save_waivers(&root, &waivers) {
        eprintln!("forall assurance waive: {err}");
        return 1;
    }
    println!("forall assurance waive: {requirement_id} waived until {expires}");
    0
}
