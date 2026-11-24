use std::collections::HashSet;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::mapping::load::mapping_path;
use crate::mapping::scan::{
    capability_from_file, discover_typescript_files, existing_code_symbol_keys,
    load_mapping_or_empty, persist_mapping, unique_requirement_id,
};
use crate::mapping::schema::{CodeRef, Requirement};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingDiscoverResult {
    pub scanned_files: usize,
    pub discovered: usize,
    pub skipped_existing: usize,
    pub skipped_no_exports: usize,
    pub added_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct MappingDiscoverOptions<'a> {
    pub root: &'a Path,
    pub dry_run: bool,
}

pub fn run_mapping_discover(
    options: MappingDiscoverOptions<'_>,
) -> Result<MappingDiscoverResult, String> {
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
    let mut result = MappingDiscoverResult {
        scanned_files: files.len(),
        ..MappingDiscoverResult::default()
    };

    for rel_file in files {
        let abs_path = options.root.join(&rel_file);
        let content = match fs::read_to_string(&abs_path) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let symbols = discover_exported_symbols(&content);
        if symbols.is_empty() {
            result.skipped_no_exports += 1;
            continue;
        }

        let imports = requirements_from_exports(
            &rel_file,
            &symbols,
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

    if result.discovered == 0 {
        return Ok(result);
    }

    if !options.dry_run {
        persist_mapping(&mapping_file, &mapping)?;
    }

    Ok(result)
}

/// Find exported function/class/const names without parsing JSX bodies.
pub fn discover_exported_symbols(source: &str) -> Vec<String> {
    let patterns = [
        r"(?m)^export\s+(?:default\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)",
        r"(?m)^export\s+(?:default\s+)?class\s+([A-Za-z_$][\w$]*)",
        r"(?m)^export\s+const\s+([A-Za-z_$][\w$]*)\s*[=:]",
        r"(?m)^export\s+let\s+([A-Za-z_$][\w$]*)\s*[=:]",
    ];

    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for pattern in patterns {
        let Ok(re) = Regex::new(pattern) else {
            continue;
        };
        for cap in re.captures_iter(source) {
            if let Some(name) = cap.get(1).map(|m| m.as_str().to_string())
                && seen.insert(name.clone())
            {
                names.push(name);
            }
        }
    }
    names
}

fn requirements_from_exports(
    rel_file: &str,
    symbols: &[String],
    existing_keys: &HashSet<(String, String)>,
    existing_ids: &HashSet<String>,
    result: &mut MappingDiscoverResult,
) -> Vec<Requirement> {
    let mut discovered = Vec::new();
    for symbol in symbols {
        let key = (rel_file.to_string(), symbol.clone());
        if existing_keys.contains(&key) {
            result.skipped_existing += 1;
            continue;
        }

        let id = unique_requirement_id(rel_file, symbol, existing_ids, &discovered);
        discovered.push(Requirement {
            id,
            capability: capability_from_file(rel_file),
            requirement: format!(
                "Discovered export `{symbol}` — spec-tracked until contract and annotations are added"
            ),
            verified: false,
            property_tested: false,
            property: None,
            code: Some(CodeRef {
                file: rel_file.to_string(),
                symbols: vec![symbol.clone()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        });
        result.discovered += 1;
    }
    discovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_tsx_component_exports() {
        let source = r#"
import React from "react";

export function Hero() {
  return <section>Namaste</section>;
}

export const Footer = () => <footer />;
"#;
        let names = discover_exported_symbols(source);
        assert!(names.contains(&"Hero".to_string()));
        assert!(names.contains(&"Footer".to_string()));
    }

    #[test]
    fn discovers_default_export_function() {
        let source = "export default function HomePage() { return null; }\n";
        let names = discover_exported_symbols(source);
        assert_eq!(names, vec!["HomePage".to_string()]);
    }

    #[test]
    fn builds_spec_tracked_requirements() {
        let mut result = MappingDiscoverResult::default();
        let reqs = requirements_from_exports(
            "src/components/Hero.tsx",
            &["Hero".to_string()],
            &HashSet::new(),
            &HashSet::new(),
            &mut result,
        );
        assert_eq!(reqs.len(), 1);
        assert!(!reqs[0].verified);
        assert_eq!(reqs[0].code.as_ref().unwrap().symbols, vec!["Hero"]);
        assert_eq!(result.discovered, 1);
    }
}
