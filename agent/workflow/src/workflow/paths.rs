use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::{
    workflow_archive_dir, workflow_archive_read_dir, workflow_changes_dir,
    workflow_changes_read_dir,
};

pub fn changes_dir(root: &Path) -> PathBuf {
    workflow_changes_dir(root)
}

pub fn change_dir(root: &Path, name: &str) -> PathBuf {
    changes_dir(root).join(name)
}

pub fn archive_dir(root: &Path) -> PathBuf {
    workflow_archive_dir(root)
}

pub fn list_active_changes(root: &Path) -> Vec<String> {
    let dir = workflow_changes_read_dir(root);
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

pub fn artifact_output_path(change_root: &Path, generates: &str) -> PathBuf {
    change_root.join(generates)
}

pub fn artifact_exists(change_root: &Path, generates: &str) -> bool {
    if generates.contains('*') {
        if generates.starts_with("specs/") {
            return dir_has_markdown(&change_root.join("specs"));
        }
        let dir = change_root.join(
            Path::new(generates)
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        );
        return dir_has_markdown(&dir);
    }
    let target = change_root.join(generates);
    target.is_file()
}

fn dir_has_markdown(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if dir_has_markdown(&path) {
                return true;
            }
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".md"))
        {
            return true;
        }
    }
    false
}

pub fn parse_tasks_progress(tasks_path: &Path) -> Option<crate::workflow::types::TasksProgress> {
    if !tasks_path.is_file() {
        return None;
    }
    let text = fs::read_to_string(tasks_path).ok()?;
    let total = text
        .lines()
        .filter(|l| {
            regex::Regex::new(r"^- \[[ xX]\]")
                .unwrap_or_else(|err| panic!("invalid task checkbox regex: {err}"))
                .is_match(l)
        })
        .count();
    let done = text
        .lines()
        .filter(|l| {
            regex::Regex::new(r"^- \[[xX]\]")
                .unwrap_or_else(|err| panic!("invalid done checkbox regex: {err}"))
                .is_match(l)
        })
        .count();
    Some(crate::workflow::types::TasksProgress { done, total })
}

pub fn archive_destination(root: &Path, name: &str) -> PathBuf {
    let date = chrono_lite_date();
    workflow_archive_read_dir(root).join(format!("{date}-{name}"))
}

fn chrono_lite_date() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = now / 86_400;
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    days += 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
