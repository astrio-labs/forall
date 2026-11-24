use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::paths::{forall_mapping_path, verify_reports_dir, workflow_changes_dir};

fn file_token(path: &Path) -> String {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return path.display().to_string(),
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}:{}:{}", path.display(), modified, meta.len())
}

pub fn check_inputs_fingerprint_path(root: &Path, change: Option<&str>) -> std::path::PathBuf {
    let stem = change.unwrap_or("project");
    verify_reports_dir(root).join(format!("{stem}-inputs.fingerprint"))
}

pub fn compute_check_inputs_fingerprint(
    root: &Path,
    change: Option<&str>,
    verified_files: &[String],
) -> String {
    let mut parts = Vec::new();
    let mapping = forall_mapping_path(root);
    if mapping.is_file() {
        parts.push(file_token(&mapping));
    }
    if let Some(change) = change {
        let delta = workflow_changes_dir(root)
            .join(change)
            .join("mapping.delta.yaml");
        if delta.is_file() {
            parts.push(file_token(&delta));
        }
    }
    for file in verified_files {
        let path = root.join(file);
        if path.is_file() {
            parts.push(file_token(&path));
        }
    }
    parts.join("|")
}

pub fn write_check_inputs_fingerprint(
    root: &Path,
    change: Option<&str>,
    verified_files: &[String],
) -> std::io::Result<()> {
    let fingerprint = compute_check_inputs_fingerprint(root, change, verified_files);
    let path = check_inputs_fingerprint_path(root, change);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{fingerprint}\n"))
}

pub fn check_inputs_unchanged(
    root: &Path,
    change: Option<&str>,
    verified_files: &[String],
) -> bool {
    let path = check_inputs_fingerprint_path(root, change);
    let stored = fs::read_to_string(path).unwrap_or_default();
    let stored = stored.trim();
    if stored.is_empty() {
        return false;
    }
    stored == compute_check_inputs_fingerprint(root, change, verified_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::{ensure_forall_layout, verify_mapping_path};

    fn setup(root: &Path) {
        ensure_forall_layout(root).unwrap();
        fs::write(verify_mapping_path(root), "version: 1\nrequirements: []\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "// verified\n").unwrap();
    }

    #[test]
    fn unchanged_when_no_fingerprint_was_ever_written() {
        let dir = tempfile::tempdir().unwrap();
        setup(dir.path());
        assert!(!check_inputs_unchanged(
            dir.path(),
            Some("my-change"),
            &["src/lib.rs".to_string()]
        ));
    }

    #[test]
    fn unchanged_after_writing_matching_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        setup(dir.path());
        let files = vec!["src/lib.rs".to_string()];
        write_check_inputs_fingerprint(dir.path(), Some("my-change"), &files).unwrap();

        assert!(check_inputs_unchanged(
            dir.path(),
            Some("my-change"),
            &files
        ));
    }

    #[test]
    fn changed_when_verified_file_content_differs_after_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        setup(dir.path());
        let files = vec!["src/lib.rs".to_string()];
        write_check_inputs_fingerprint(dir.path(), Some("my-change"), &files).unwrap();

        fs::write(
            dir.path().join("src/lib.rs"),
            "// edited, different length now\n",
        )
        .unwrap();

        assert!(!check_inputs_unchanged(
            dir.path(),
            Some("my-change"),
            &files
        ));
    }

    #[test]
    fn changed_when_mapping_differs_after_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        setup(dir.path());
        let files = vec!["src/lib.rs".to_string()];
        write_check_inputs_fingerprint(dir.path(), Some("my-change"), &files).unwrap();

        fs::write(
            verify_mapping_path(dir.path()),
            "version: 1\nrequirements:\n  - id: new-req\n",
        )
        .unwrap();

        assert!(!check_inputs_unchanged(
            dir.path(),
            Some("my-change"),
            &files
        ));
    }

    #[test]
    fn fingerprint_is_scoped_per_change() {
        let dir = tempfile::tempdir().unwrap();
        setup(dir.path());
        let files = vec!["src/lib.rs".to_string()];
        write_check_inputs_fingerprint(dir.path(), Some("change-a"), &files).unwrap();

        assert!(check_inputs_unchanged(dir.path(), Some("change-a"), &files));
        assert!(!check_inputs_unchanged(
            dir.path(),
            Some("change-b"),
            &files
        ));
    }

    #[test]
    fn fingerprint_detects_verified_file_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".forall/verify")).unwrap();
        std::fs::write(root.join(".forall/verify/mapping.yaml"), "version: 1\n").unwrap();
        std::fs::write(root.join("Game.java"), "class Game {}\n").unwrap();

        let files = vec!["Game.java".to_string()];
        write_check_inputs_fingerprint(root, None, &files).unwrap();
        assert!(check_inputs_unchanged(root, None, &files));

        std::fs::write(root.join("Game.java"), "class Game { void move() {} }\n").unwrap();
        assert!(!check_inputs_unchanged(root, None, &files));
    }
}
