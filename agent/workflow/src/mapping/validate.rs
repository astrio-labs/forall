use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::mapping::schema::{IssueSeverity, Requirement, VerifyIssue};

#[derive(Debug, Clone)]
pub struct ExtractedFunction {
    pub name: String,
    pub requires: Vec<String>,
    pub ensures: Vec<String>,
    pub contract: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub functions: Vec<ExtractedFunction>,
}

pub fn validate_mapping_structure(root: &Path, requirements: &[Requirement]) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();

    for req in requirements {
        if !req.verified {
            continue;
        }

        let Some(code) = &req.code else {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "mapping".to_string(),
                requirement_id: Some(req.id.clone()),
                message: "verified requirement missing code.file and code.symbols".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            continue;
        };

        let file_path = root.join(&code.file);
        if !file_path.is_file() {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "mapping".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!("code file not found: {}", code.file),
                file: Some(code.file.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
    }

    issues
}

pub fn validate_symbols_against_extract(
    root: &Path,
    requirements: &[Requirement],
    extracts: &HashMap<String, ExtractResult>,
) -> Vec<VerifyIssue> {
    let _ = root;
    let mut issues = Vec::new();

    for req in requirements {
        if !req.verified {
            continue;
        }
        let Some(code) = &req.code else {
            continue;
        };

        let Some(extract) = extracts.get(&code.file) else {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "mapping".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!("could not extract symbols from {}", code.file),
                file: Some(code.file.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
            continue;
        };

        let names: HashSet<_> = extract.functions.iter().map(|f| f.name.as_str()).collect();
        for symbol in &code.symbols {
            if !names.contains(symbol.as_str()) {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Critical,
                    phase: "mapping".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!("symbol '{symbol}' not found in {}", code.file),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }

        for symbol in &code.symbols {
            let Some(fn_def) = extract.functions.iter().find(|f| &f.name == symbol) else {
                continue;
            };
            if fn_def.requires.is_empty() && fn_def.ensures.is_empty() {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "mapping".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!("symbol '{symbol}' has no //@ requires or //@ ensures"),
                    file: Some(code.file.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }
    }

    issues
}

/// Files whose requirements intend formal proof, for a mapping of `version`.
///
/// Version 1 says so with the writable `verified` bool, and says it in both
/// directions: `verified: false` beside a finished contract is a deliberate
/// disarm, so there the bool stays authoritative. Version 2 has no claim
/// bools to read, so intent is the artifact — an attached contract. Reading
/// only the bool made every v2 mapping proof-ineligible: `forall mapping
/// migrate` produced a project that could never earn Proved again.
pub fn verified_files_from_requirements(requirements: &[Requirement], version: u32) -> Vec<String> {
    let mut files = HashSet::new();
    for req in requirements {
        let intends_proof = if version >= 2 {
            crate::mapping::schema::claimed_evidence_level(req, version)
                >= crate::mapping::schema::EvidenceLevel::Contracted
        } else {
            req.verified
        };
        if intends_proof && let Some(code) = &req.code {
            files.insert(code.file.clone());
        }
    }
    let mut sorted: Vec<_> = files.into_iter().collect();
    sorted.sort();
    sorted
}

#[cfg(test)]
mod proof_eligibility_tests {
    use super::*;
    use crate::mapping::schema::CodeRef;

    fn contracted(id: &str, file: &str) -> Requirement {
        Requirement {
            id: id.to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified: false,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: file.to_string(),
                symbols: vec!["f".to_string()],
            }),
            contract: Some("result >= 0".to_string()),
            claimcheck: None,
            scenarios: None,
        }
    }

    /// The regression that made `forall mapping migrate` a one-way door: a
    /// migrated mapping has no `verified` bool to read, so every file fell
    /// out of proof eligibility and the project could never earn Proved.
    #[test]
    fn a_version_2_contract_makes_its_file_proof_eligible() {
        let requirements = vec![contracted("REQ-1", "src/pricing.ts")];

        assert!(!requirements[0].verified);
        assert_eq!(
            verified_files_from_requirements(&requirements, 2),
            vec!["src/pricing.ts".to_string()]
        );
    }

    /// Intent is the contract, not the citation: a requirement that only
    /// tracks a file must not drag that file into the prover.
    #[test]
    fn a_bare_citation_stays_out_of_the_prover() {
        let mut req = contracted("REQ-1", "src/pricing.ts");
        req.contract = None;

        assert!(verified_files_from_requirements(&[req], 2).is_empty());
    }
}
