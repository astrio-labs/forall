use std::collections::HashMap;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::mapping::schema::Requirement;
use crate::verify::adapters::file_extension;
use crate::verify::engine::lsc_extract;

#[derive(Debug, Clone, Default)]
pub struct AnnotationSyncResult {
    pub files_updated: Vec<String>,
    pub stubs_added: usize,
    pub skipped: Vec<String>,
}

fn escape_regexp(value: &str) -> String {
    regex::escape(value)
}

fn find_function_line_index(lines: &[String], symbol: &str) -> Option<usize> {
    let patterns = [
        format!(
            r"^\s*(?:export\s+)?(?:async\s+)?function\s+{}\s*\(",
            escape_regexp(symbol)
        ),
        format!(
            r"^\s*(?:export\s+)?(?:const|let|var)\s+{}\s*=\s*(?:async\s*)?\(",
            escape_regexp(symbol)
        ),
        format!(
            r"^\s*(?:export\s+)?(?:const|let|var)\s+{}\s*=\s*(?:async\s+)?function",
            escape_regexp(symbol)
        ),
    ];
    let compiled: Vec<Regex> = patterns
        .iter()
        .map(|p| {
            Regex::new(p).unwrap_or_else(|err| panic!("invalid function pattern regex: {err}"))
        })
        .collect();
    for (i, line) in lines.iter().enumerate() {
        if compiled.iter().any(|re| re.is_match(line)) {
            return Some(i);
        }
    }
    None
}

fn indent_for_line(line: &str) -> String {
    Regex::new(r"^(\s*)")
        .unwrap_or_else(|err| panic!("invalid indent regex: {err}"))
        .captures(line)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

fn build_stub_lines(
    indent: &str,
    needs_requires: bool,
    needs_ensures: bool,
    needs_contract: bool,
    contract: Option<&str>,
) -> Vec<String> {
    let mut lines = Vec::new();
    if needs_contract && let Some(contract) = contract {
        let first_line = contract.trim().lines().next().unwrap_or("");
        lines.push(format!("{indent}//@ contract {first_line}"));
    }
    if needs_requires {
        lines.push(format!(
            "{indent}//@ requires /* TODO: from verification plan */"
        ));
    }
    if needs_ensures {
        lines.push(format!(
            "{indent}//@ ensures /* TODO: from verification plan */"
        ));
    }
    lines
}

pub fn sync_annotations(root: &Path, requirements: &[Requirement]) -> AnnotationSyncResult {
    let mut files_updated = Vec::new();
    let mut skipped = Vec::new();
    let mut stubs_added = 0usize;

    let mut by_file: HashMap<String, Vec<&Requirement>> = HashMap::new();
    for req in requirements {
        if !req.verified {
            continue;
        }
        let Some(code) = &req.code else {
            continue;
        };
        by_file.entry(code.file.clone()).or_default().push(req);
    }

    for (rel_file, reqs) in by_file {
        if file_extension(&rel_file) != Some("ts") && file_extension(&rel_file) != Some("tsx") {
            skipped.push(format!("{rel_file} (annotation sync is TypeScript-only)"));
            continue;
        }
        let abs_path = root.join(&rel_file);
        if !abs_path.is_file() {
            skipped.push(format!("{rel_file} (file missing)"));
            continue;
        }

        let extract = lsc_extract(root, &rel_file);
        let content = fs::read_to_string(&abs_path).unwrap_or_default();
        let mut lines: Vec<String> = content.split('\n').map(str::to_string).collect();
        let mut file_changed = false;

        for req in reqs {
            let Some(code) = req.code.as_ref() else {
                continue;
            };
            for symbol in &code.symbols {
                let fn_def = extract
                    .as_ref()
                    .and_then(|e| e.functions.iter().find(|f| &f.name == symbol));
                let needs_requires = fn_def.is_none_or(|f| f.requires.is_empty());
                let needs_ensures = fn_def.is_none_or(|f| f.ensures.is_empty());
                let needs_contract =
                    req.contract.is_some() && fn_def.is_none_or(|f| f.contract.is_empty());

                if !needs_requires && !needs_ensures && !needs_contract {
                    continue;
                }

                let Some(line_index) = find_function_line_index(&lines, symbol) else {
                    skipped.push(format!("{rel_file}::{symbol} (symbol not found)"));
                    continue;
                };

                let indent = indent_for_line(&lines[line_index]);
                let stub_lines = build_stub_lines(
                    &indent,
                    needs_requires,
                    needs_ensures,
                    needs_contract,
                    req.contract.as_deref(),
                );
                if stub_lines.is_empty() {
                    continue;
                }

                for (offset, stub) in stub_lines.iter().enumerate() {
                    lines.insert(line_index + offset, stub.clone());
                }
                stubs_added += stub_lines.len();
                file_changed = true;
            }
        }

        if file_changed {
            fs::write(&abs_path, lines.join("\n")).ok();
            files_updated.push(rel_file);
        }
    }

    AnnotationSyncResult {
        files_updated,
        stubs_added,
        skipped,
    }
}
