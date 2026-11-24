use std::collections::HashSet;
use std::path::Path;

use crate::mapping::load::mapping_path;
use crate::mapping::scan::{
    capability_from_file, discover_typescript_files, existing_code_symbol_keys,
    load_mapping_or_empty, persist_mapping, unique_requirement_id,
};
use crate::mapping::schema::{CodeRef, Requirement};
use crate::mapping::validate::{ExtractResult, ExtractedFunction};
use crate::verify::engine::lsc_extract;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingImportResult {
    pub scanned_files: usize,
    pub imported: usize,
    pub skipped_existing: usize,
    pub skipped_no_annotations: usize,
    pub extract_failures: usize,
    pub added_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MappingImportOptions<'a> {
    pub root: &'a Path,
    pub dry_run: bool,
}

pub fn run_mapping_import(
    options: MappingImportOptions<'_>,
) -> Result<MappingImportResult, String> {
    let mapping_file = mapping_path(options.root);
    if !mapping_file.is_file() {
        return Err(
            "mapping.yaml not found — run `forall init` in the project root first".to_string(),
        );
    }

    let mut mapping = load_mapping_or_empty(&mapping_file)?;
    let mut existing_keys = existing_code_symbol_keys(&mapping.requirements);
    let mut existing_ids: HashSet<String> =
        mapping.requirements.iter().map(|r| r.id.clone()).collect();

    let files = discover_typescript_files(options.root);
    let mut result = MappingImportResult {
        scanned_files: files.len(),
        ..MappingImportResult::default()
    };

    for rel_file in files {
        let Some(extract) = lsc_extract(options.root, &rel_file) else {
            result.extract_failures += 1;
            continue;
        };
        let imports = requirements_from_extract(
            &rel_file,
            &extract,
            &existing_keys,
            &existing_ids,
            &mut result,
        );
        for req in imports {
            if let Some(code) = &req.code {
                for symbol in &code.symbols {
                    existing_keys.insert((code.file.clone(), symbol.clone()));
                }
            }
            existing_ids.insert(req.id.clone());
            result.added_ids.push(req.id.clone());
            mapping.requirements.push(req);
        }
    }

    if result.imported == 0 {
        return Ok(result);
    }

    if !options.dry_run {
        persist_mapping(&mapping_file, &mapping)?;
    }

    Ok(result)
}

pub fn requirements_from_extract(
    rel_file: &str,
    extract: &ExtractResult,
    existing_keys: &HashSet<(String, String)>,
    existing_ids: &HashSet<String>,
    result: &mut MappingImportResult,
) -> Vec<Requirement> {
    let mut imported = Vec::new();
    for function in &extract.functions {
        if !has_annotations(function) {
            result.skipped_no_annotations += 1;
            continue;
        }
        let key = (rel_file.to_string(), function.name.clone());
        if existing_keys.contains(&key) {
            result.skipped_existing += 1;
            continue;
        }

        let id = unique_requirement_id(rel_file, &function.name, existing_ids, &imported);
        let requirement_text = requirement_text_from_extract(function);
        let contract = contract_from_extract(function);

        imported.push(Requirement {
            id,
            capability: capability_from_file(rel_file),
            requirement: requirement_text,
            verified: verified_from_extract(function),
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: rel_file.to_string(),
                symbols: vec![function.name.clone()],
            }),
            contract,
            claimcheck: None,
            scenarios: None,
        });
        result.imported += 1;
    }
    imported
}

fn has_annotations(function: &ExtractedFunction) -> bool {
    !function.requires.is_empty() || !function.ensures.is_empty() || !function.contract.is_empty()
}

fn verified_from_extract(function: &ExtractedFunction) -> bool {
    !function.requires.is_empty() || !function.ensures.is_empty()
}

fn requirement_text_from_extract(function: &ExtractedFunction) -> String {
    if let Some(line) = function.contract.first() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    format!("Imported annotation coverage for `{}`", function.name)
}

fn contract_from_extract(function: &ExtractedFunction) -> Option<String> {
    if function.contract.is_empty() {
        return None;
    }
    Some(function.contract.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::validate::ExtractedFunction;

    #[test]
    fn builds_requirement_from_annotated_function() {
        let extract = ExtractResult {
            functions: vec![ExtractedFunction {
                name: "clamp".to_string(),
                requires: vec!["lo <= hi".to_string()],
                ensures: vec!["result >= lo".to_string()],
                contract: vec!["Bounds clamp".to_string()],
            }],
        };
        let existing_keys = HashSet::new();
        let existing_ids = HashSet::new();
        let mut result = MappingImportResult::default();
        let reqs = requirements_from_extract(
            "src/math/clamp.ts",
            &extract,
            &existing_keys,
            &existing_ids,
            &mut result,
        );
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].id, "clamp-clamp");
        assert!(reqs[0].verified);
        assert_eq!(reqs[0].capability, "math");
        assert_eq!(result.imported, 1);
    }

    #[test]
    fn skips_functions_without_annotations() {
        let extract = ExtractResult {
            functions: vec![ExtractedFunction {
                name: "plain".to_string(),
                requires: Vec::new(),
                ensures: Vec::new(),
                contract: Vec::new(),
            }],
        };
        let mut result = MappingImportResult::default();
        let reqs = requirements_from_extract(
            "src/plain.ts",
            &extract,
            &HashSet::new(),
            &HashSet::new(),
            &mut result,
        );
        assert!(reqs.is_empty());
        assert_eq!(result.skipped_no_annotations, 1);
    }

    #[test]
    fn skips_existing_file_symbol_pairs() {
        let extract = ExtractResult {
            functions: vec![ExtractedFunction {
                name: "demo".to_string(),
                requires: vec!["true".to_string()],
                ensures: Vec::new(),
                contract: Vec::new(),
            }],
        };
        let existing_keys = HashSet::from([("src/demo.ts".to_string(), "demo".to_string())]);
        let mut result = MappingImportResult::default();
        let reqs = requirements_from_extract(
            "src/demo.ts",
            &extract,
            &existing_keys,
            &HashSet::new(),
            &mut result,
        );
        assert!(reqs.is_empty());
        assert_eq!(result.skipped_existing, 1);
    }
}
