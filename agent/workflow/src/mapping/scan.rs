use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;

use crate::mapping::schema::{Mapping, Requirement, validate_mapping};
use crate::paths::ensure_dir;

pub const SCAN_ROOTS: &[&str] = &["src", "lib", "app"];

pub fn discover_typescript_files(root: &Path) -> Vec<String> {
    let mut files = BTreeSet::new();
    for scan_root in SCAN_ROOTS {
        walk_typescript_files(&root.join(scan_root), root, &mut files);
    }
    files.into_iter().collect()
}

fn walk_typescript_files(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
    if !dir.is_dir() {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if should_skip_dir(path.file_name().and_then(|name| name.to_str())) {
                continue;
            }
            walk_typescript_files(&path, root, out);
            continue;
        }
        if is_typescript_source_file(&path) {
            out.insert(rel_path_from_root(root, &path));
        }
    }
}

fn should_skip_dir(name: Option<&str>) -> bool {
    matches!(
        name,
        Some("node_modules" | ".git" | "target" | "dist" | "build" | ".forall" | "archive")
    )
}

fn is_typescript_source_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if file_name.ends_with(".d.ts") {
        return false;
    }
    if file_name.contains(".test.") || file_name.contains(".spec.") {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext == "ts" || ext == "tsx")
}

fn rel_path_from_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

pub fn load_mapping_or_empty(path: &Path) -> Result<Mapping, String> {
    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    if raw.trim().is_empty() {
        return Ok(Mapping {
            version: 1,
            requirements: Vec::new(),
        });
    }
    let mapping: Mapping = serde_yaml::from_str(&raw).map_err(|err| err.to_string())?;
    validate_mapping(&mapping)?;
    Ok(mapping)
}

pub fn persist_mapping(path: &Path, mapping: &Mapping) -> Result<(), String> {
    validate_mapping(mapping)?;
    if let Some(parent) = path.parent() {
        ensure_dir(parent).map_err(|err| err.to_string())?;
    }
    fs::write(
        path,
        serde_yaml::to_string(mapping).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn existing_code_symbol_keys(requirements: &[Requirement]) -> HashSet<(String, String)> {
    requirements
        .iter()
        .filter_map(|req| {
            let code = req.code.as_ref()?;
            Some(
                code.symbols
                    .iter()
                    .map(|symbol| (code.file.clone(), symbol.clone()))
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect()
}

pub fn capability_from_file(rel_file: &str) -> String {
    let parts: Vec<&str> = rel_file.split('/').collect();
    if parts.len() >= 2 && parts[0] == "src" && !parts[1].is_empty() {
        parts[1].to_string()
    } else {
        "discovered".to_string()
    }
}

pub fn unique_requirement_id(
    rel_file: &str,
    symbol: &str,
    existing_ids: &HashSet<String>,
    pending: &[Requirement],
) -> String {
    let stem = Path::new(rel_file)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let base = format!("{}-{}", slug(stem), slug(symbol));
    let mut candidate = base.clone();
    let mut suffix = 2u32;
    let pending_ids: HashSet<_> = pending.iter().map(|req| req.id.as_str()).collect();
    while existing_ids.contains(&candidate) || pending_ids.contains(candidate.as_str()) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    candidate
}

pub fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_declaration_and_test_files_by_name() {
        assert!(!is_typescript_source_file(Path::new("src/vite-env.d.ts")));
        assert!(!is_typescript_source_file(Path::new("src/foo.test.ts")));
        assert!(is_typescript_source_file(Path::new(
            "src/components/Hero.tsx"
        )));
    }
}
