use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::paths::find_project_root;

pub fn workspace_root(input: &Value) -> PathBuf {
    let base = input
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| Path::new(s).to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    base.canonicalize().unwrap_or(base)
}

pub fn project_root(input: &Value) -> Option<PathBuf> {
    find_project_root(&workspace_root(input))
}
