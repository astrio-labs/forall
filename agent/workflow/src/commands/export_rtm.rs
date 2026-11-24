//! `forall export rtm`: the requirements-traceability matrix, generated.
//!
//! Regulated teams (IEC 62304, ISO 26262, DO-178C) produce requirement →
//! code → verification-evidence matrices by hand today. Every column
//! already lives in the mapping and the obligation ledger; this command is
//! the join. Levels come from the ledger's EARNED rungs — the RTM states
//! what was demonstrated, never what was claimed.

use std::fs;
use std::path::Path;

use crate::mapping::load::requirements_for_check;
use crate::paths::find_project_root;
use crate::verify::ledger::{Ledger, read_ledger};

fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub fn render_rtm(requirements_text: &[(String, String, String)], ledger: &Ledger) -> String {
    let mut lines = vec![
        "requirement_id,capability,requirement,file,symbols,claimed_level,earned_level,tools,statuses".to_string(),
    ];
    for entry in &ledger.entries {
        let (capability, text) = requirements_text
            .iter()
            .find(|(id, _, _)| id == &entry.requirement_id)
            .map(|(_, capability, text)| (capability.as_str(), text.as_str()))
            .unwrap_or(("", ""));
        let (file, symbols) = entry
            .code
            .as_ref()
            .map(|code| (code.file.as_str(), code.symbols.join(" ")))
            .unwrap_or(("", String::new()));
        let mut tools: Vec<String> = entry
            .obligations
            .iter()
            .map(|obligation| match &obligation.tool_version {
                Some(version) => format!("{} {version}", obligation.tool),
                None => obligation.tool.clone(),
            })
            .collect();
        tools.dedup();
        let mut statuses: Vec<&str> = entry
            .obligations
            .iter()
            .map(|obligation| match obligation.status {
                crate::verify::ledger::ObligationStatus::Proved => "proved",
                crate::verify::ledger::ObligationStatus::Failed => "failed",
                crate::verify::ledger::ObligationStatus::ToolUnavailable => "tool_unavailable",
            })
            .collect();
        statuses.dedup();
        lines.push(
            [
                csv_escape(&entry.requirement_id),
                csv_escape(capability),
                csv_escape(text),
                csv_escape(file),
                csv_escape(&symbols),
                entry.claimed_level.as_str().to_string(),
                entry.earned_level.as_str().to_string(),
                csv_escape(&tools.join("; ")),
                csv_escape(&statuses.join("; ")),
            ]
            .join(","),
        );
    }
    lines.join("\n") + "\n"
}

pub fn run_export_rtm(root_arg: Option<&Path>, change: Option<&str>, out: Option<&Path>) -> i32 {
    let cwd = root_arg
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall export rtm: project not found");
        return 1;
    };
    let Some(ledger) = read_ledger(&root, change) else {
        eprintln!(
            "forall export rtm: no verify ledger found — run `forall check` first \
             (a missing or tampered ledger reads as no evidence)"
        );
        return 1;
    };
    let requirements_text: Vec<(String, String, String)> = requirements_for_check(&root, change)
        .unwrap_or_default()
        .into_iter()
        .map(|req| (req.id, req.capability, req.requirement))
        .collect();

    let csv = render_rtm(&requirements_text, &ledger);
    match out {
        Some(path) => {
            if let Err(err) = fs::write(path, csv) {
                eprintln!("forall export rtm: write failed: {err}");
                return 1;
            }
            println!("forall export rtm: wrote {}", path.display());
        }
        None => print!("{csv}"),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::{CodeRef, Requirement};
    use crate::verify::ledger::{ObligationStatus, ProofOutcome, build_ledger};

    #[test]
    fn rtm_joins_ledger_and_requirement_text_with_csv_escaping() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/pay.c"), "int f(void){return 0;}\n").unwrap();
        let requirements = vec![Requirement {
            id: "PAY-1".to_string(),
            capability: "payments".to_string(),
            requirement: "Refunds, when partial, \"round down\".".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: "src/pay.c".to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        }];
        let ledger = build_ledger(
            dir.path(),
            None,
            &requirements,
            &[ProofOutcome {
                file: "src/pay.c".to_string(),
                tool: "framac".to_string(),
                tool_version: Some("Frama-C 32.1".to_string()),
                status: ObligationStatus::Proved,
            }],
        );
        let text: Vec<(String, String, String)> = requirements
            .iter()
            .map(|r| (r.id.clone(), r.capability.clone(), r.requirement.clone()))
            .collect();

        let csv = render_rtm(&text, &ledger);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("requirement_id,capability,"));
        let row = lines[1];
        assert!(row.starts_with("PAY-1,payments,"));
        // Embedded comma and quotes force CSV escaping.
        assert!(row.contains("\"Refunds, when partial, \"\"round down\"\".\""));
        assert!(row.contains("src/pay.c"));
        assert!(row.contains(",proved,proved,"));
        assert!(row.contains("framac Frama-C 32.1"));
    }
}
