use std::fs;
use std::path::Path;

use regex::Regex;
use std::sync::LazyLock;

static HEADER_ONLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#+\s").unwrap_or_else(|err| panic!("invalid header regex: {err}"))
});
static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<!--[\s\S]*?-->").unwrap_or_else(|err| panic!("invalid HTML comment regex: {err}"))
});

/// True when markdown is only headers and HTML comment placeholders.
pub fn is_placeholder_markdown(text: &str) -> bool {
    let without_comments = HTML_COMMENT.replace_all(text, "").trim().to_string();
    if without_comments.is_empty() {
        return true;
    }
    !without_comments
        .lines()
        .map(str::trim)
        .any(|line| !line.is_empty() && !HEADER_ONLY.is_match(line))
}

pub fn markdown_file_has_content(file_path: &Path) -> bool {
    if !file_path.is_file() {
        return false;
    }
    let Ok(text) = fs::read_to_string(file_path) else {
        return false;
    };
    !is_placeholder_markdown(&text)
}

pub fn specs_dir_has_content(specs_dir: &Path) -> bool {
    if !specs_dir.is_dir() {
        return false;
    }
    let Ok(entries) = fs::read_dir(specs_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let spec_file = entry.path().join("spec.md");
        if spec_file.is_file() && markdown_file_has_content(&spec_file) {
            return true;
        }
    }
    false
}
