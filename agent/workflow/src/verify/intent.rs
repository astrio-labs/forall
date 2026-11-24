use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::mapping::schema::{IssueSeverity, Requirement, VerifyIssue};
use crate::mapping::validate::ExtractResult;
use crate::paths::verify_intent_dir;

fn normalize_text(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Normalize contract text for heuristic alignment (mapping vs extracted code).
fn normalize_contract_text(text: &str, is_jml: bool) -> String {
    let mut prepared = text.to_string();
    if is_jml {
        prepared = prepared.replace("\\result", "result");
        prepared = prepared.replace("\\old", "old");
        prepared = prepared.replace("//@", " ");
        prepared = prepared.replace("&&", " ");
        prepared = prepared.replace(';', " ");
        prepared = prepared.split_whitespace().collect::<Vec<_>>().join(" ");
        while prepared.contains("result result") {
            prepared = prepared.replace("result result", "result");
        }
    }
    normalize_text(&prepared)
}

fn contract_lines(text: Option<&str>) -> Vec<String> {
    text.map(|t| {
        t.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

fn contracts_align(mapping_contract: &str, code_contracts: &[String], is_jml: bool) -> bool {
    if code_contracts.is_empty() {
        return false;
    }
    let mapping_norm = normalize_contract_text(mapping_contract, is_jml);
    let code_norm = normalize_contract_text(&code_contracts.join(" "), is_jml);
    if mapping_norm == code_norm {
        return true;
    }
    mapping_norm.contains(&code_norm) || code_norm.contains(&mapping_norm)
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentClaimcheckRecord {
    pub requirement_id: String,
    pub symbol: String,
    pub file: String,
    pub mapping_contract: String,
    pub code_contract: Vec<String>,
    pub aligned: bool,
    pub claimcheck_requested: bool,
    pub claimcheck_ran: bool,
    pub note: String,
}

pub fn validate_intent(
    _root: &Path,
    requirements: &[Requirement],
    extracts: &HashMap<String, ExtractResult>,
) -> (Vec<VerifyIssue>, Vec<IntentClaimcheckRecord>) {
    let mut issues = Vec::new();
    let mut records = Vec::new();

    for req in requirements {
        if !req.verified {
            continue;
        }
        let Some(code) = &req.code else {
            continue;
        };

        let mapping_lines = contract_lines(req.contract.as_deref());
        let extract = extracts.get(&code.file);
        let is_jml = code.file.ends_with(".java");

        let mut symbol_contracts: Vec<(String, Vec<String>)> = Vec::new();
        let mut all_code_contracts: Vec<String> = Vec::new();
        for symbol in &code.symbols {
            let fn_def = extract.and_then(|e| e.functions.iter().find(|f| &f.name == symbol));
            let code_contract = fn_def.map(|f| f.contract.clone()).unwrap_or_default();
            all_code_contracts.extend(code_contract.iter().cloned());
            symbol_contracts.push((symbol.clone(), code_contract));
        }

        let requirement_aligned = if mapping_lines.is_empty() && all_code_contracts.is_empty() {
            true
        } else if mapping_lines.is_empty() || all_code_contracts.is_empty() {
            false
        } else {
            contracts_align(&mapping_lines.join(" "), &all_code_contracts, is_jml)
        };

        let mut emitted_alignment_warning = false;

        for (symbol, code_contract) in symbol_contracts {
            let mut record = IntentClaimcheckRecord {
                requirement_id: req.id.clone(),
                symbol: symbol.clone(),
                file: code.file.clone(),
                mapping_contract: req.contract.clone().unwrap_or_default(),
                code_contract: code_contract.clone(),
                aligned: false,
                claimcheck_requested: req.claimcheck.unwrap_or(false),
                claimcheck_ran: false,
                note: String::new(),
            };

            if mapping_lines.is_empty() && code_contract.is_empty() {
                record.aligned = true;
                record.note = "no contract declared in mapping or code".to_string();
                records.push(record);
                continue;
            }

            if !mapping_lines.is_empty() && code_contract.is_empty() {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "intent".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!(
                        "symbol '{symbol}' missing //@ contract (mapping.contract is set)"
                    ),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
                record.note = "missing //@ contract annotation".to_string();
                records.push(record);
                continue;
            }

            if mapping_lines.is_empty() && !code_contract.is_empty() {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "intent".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!(
                        "symbol '{symbol}' has //@ contract but mapping.contract is empty"
                    ),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
                record.note = "mapping.contract missing".to_string();
                records.push(record);
                continue;
            }

            record.aligned = requirement_aligned;

            if !requirement_aligned && !emitted_alignment_warning {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "intent".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: "//@ contracts do not align with mapping.contract".to_string(),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
                record.note = "contract text mismatch (heuristic)".to_string();
                emitted_alignment_warning = true;
            } else if requirement_aligned && req.claimcheck.unwrap_or(false) {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Info,
                    phase: "intent".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!(
                        "claimcheck requested for '{symbol}' — full claimcheck tool not bundled; heuristic alignment passed"
                    ),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
                record.note = "heuristic alignment passed; claimcheck tool not run".to_string();
            } else if requirement_aligned {
                record.note = "heuristic alignment passed".to_string();
            }

            records.push(record);
        }
    }

    (issues, records)
}

pub fn write_claimcheck_records(
    root: &Path,
    change: Option<&str>,
    records: &[IntentClaimcheckRecord],
) -> std::io::Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let dir = verify_intent_dir(root);
    fs::create_dir_all(&dir)?;
    let stem = change.unwrap_or("project");
    let payload = serde_json::json!({
        "generatedAt": chrono_iso_now(),
        "records": records,
    });
    fs::write(
        dir.join(format!("{stem}-intent.json")),
        format!("{}\n", serde_json::to_string_pretty(&payload)?),
    )
}

fn chrono_iso_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

pub fn intent_phase_status(
    requirements: &[Requirement],
    issues: &[VerifyIssue],
) -> crate::mapping::schema::PhaseStatus {
    use crate::mapping::schema::PhaseStatus;
    let verified = requirements
        .iter()
        .filter(|r| r.verified && r.code.is_some())
        .count();
    if verified == 0 {
        return PhaseStatus::Skip;
    }
    if issues
        .iter()
        .any(|i| i.phase == "intent" && i.severity == IssueSeverity::Critical)
    {
        return PhaseStatus::Fail;
    }
    PhaseStatus::Pass
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::{CodeRef, Requirement};
    use crate::mapping::validate::{ExtractResult, ExtractedFunction};

    #[test]
    fn jml_contracts_align_result_shorthand_with_backslash_result() {
        let mapping = "ensures lo <= result <= hi";
        let code = vec![
            "//@ requires lo <= hi".to_string(),
            r#"//@ ensures lo <= \result && \result <= hi"#.to_string(),
        ];
        assert!(contracts_align(mapping, &code, true));
    }

    #[test]
    fn non_jml_contracts_do_not_apply_jml_normalization() {
        let mapping = "ensures x == 1";
        let code = vec!["//@ ensures x == 2".to_string()];
        assert!(!contracts_align(mapping, &code, false));
    }

    #[test]
    fn multi_symbol_requirement_aligns_against_union_of_contracts() {
        let req = Requirement {
            id: "req-clamp-move".to_string(),
            capability: "game".to_string(),
            requirement: "clamp and move".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: "Game.java".to_string(),
                symbols: vec!["clamp".to_string(), "move".to_string()],
            }),
            contract: Some("ensures lo <= result <= hi".to_string()),
            claimcheck: None,
            scenarios: None,
        };
        let mut extracts = HashMap::new();
        extracts.insert(
            "Game.java".to_string(),
            ExtractResult {
                functions: vec![
                    ExtractedFunction {
                        name: "clamp".to_string(),
                        requires: Vec::new(),
                        ensures: Vec::new(),
                        contract: vec![r#"//@ ensures lo <= \result && \result <= hi"#.to_string()],
                    },
                    ExtractedFunction {
                        name: "move".to_string(),
                        requires: Vec::new(),
                        ensures: Vec::new(),
                        contract: vec!["//@ requires lo <= hi".to_string()],
                    },
                ],
            },
        );

        let (issues, records) = validate_intent(Path::new("/tmp"), &[req], &extracts);
        let alignment_warnings: Vec<_> = issues
            .iter()
            .filter(|i| i.message.contains("do not align"))
            .collect();
        assert!(
            alignment_warnings.is_empty(),
            "expected no per-symbol alignment warnings: {alignment_warnings:?}"
        );
        assert!(records.iter().all(|r| r.aligned));
    }
}
