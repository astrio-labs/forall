use std::env;
use std::fs;
use std::path::Path;

use crate::paths::{FORALL_DIR, workflow_active_path, workflow_active_read_path};
use crate::workflow::paths::list_active_changes;

pub fn active_change_file() -> String {
    format!("{FORALL_DIR}/workflow/active")
}

pub fn read_active_change_file(root: &Path) -> Option<String> {
    let file_path = workflow_active_read_path(root);
    if !file_path.is_file() {
        return None;
    }
    let name = fs::read_to_string(file_path).ok()?.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

pub fn write_active_change_file(root: &Path, name: &str) -> std::io::Result<()> {
    let file_path = workflow_active_path(root);
    if let Some(parent) = file_path.parent() {
        crate::paths::ensure_dir(parent)?;
    }
    fs::write(file_path, format!("{name}\n"))
}

pub fn resolve_active_change(root: &Path) -> Option<String> {
    if let Ok(from_env) = env::var("FORALL_CHANGE") {
        let trimmed = from_env.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(from_file) = read_active_change_file(root) {
        let active = list_active_changes(root);
        if active.iter().any(|c| c == &from_file) {
            return Some(from_file);
        }
    }

    let changes = list_active_changes(root);
    if changes.len() == 1 {
        return Some(changes[0].clone());
    }

    None
}

pub fn resolve_active_change_or_throw(root: &Path) -> Result<String, String> {
    match resolve_active_change(root) {
        Some(change) => Ok(change),
        None => {
            let n = list_active_changes(root).len();
            Err(if n == 0 {
                "no active change (run `forall propose <name>`)".to_string()
            } else {
                format!(
                    "multiple active changes — set FORALL_CHANGE or write {FORALL_DIR}/workflow/active"
                )
            })
        }
    }
}
