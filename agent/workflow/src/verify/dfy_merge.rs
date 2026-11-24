use std::fs;
use std::path::Path;

use regex::Regex;
use std::sync::LazyLock;

use crate::diagnostic::format_diagnostic_output;
use crate::mapping::schema::{IssueSeverity, VerifyIssue};
use crate::verify::engine::{LscRunResult, lsc_regen};

static CONFLICT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^<<<<<<< ")
        .unwrap_or_else(|err| panic!("invalid conflict marker regex: {err}"))
});
static CONFLICT_EQUALS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^=======\s*$")
        .unwrap_or_else(|err| panic!("invalid conflict equals regex: {err}"))
});
static CONFLICT_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^>>>>>>> ").unwrap_or_else(|err| panic!("invalid conflict end regex: {err}"))
});

pub fn has_merge_conflict_markers(content: &str) -> bool {
    CONFLICT_MARKER.is_match(content)
        || CONFLICT_EQUALS.is_match(content)
        || CONFLICT_END.is_match(content)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfyArtifactPaths {
    pub dfy: String,
    pub r#gen: String,
    pub base: String,
    pub merged: String,
}

pub fn dfy_artifact_paths(rel_ts_file: &str) -> DfyArtifactPaths {
    let path = Path::new(rel_ts_file);
    let rel_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let join = |suffix: &str| -> String {
        if rel_dir == Path::new(".") {
            format!("{stem}{suffix}")
        } else {
            rel_dir
                .join(format!("{stem}{suffix}"))
                .display()
                .to_string()
        }
    };
    DfyArtifactPaths {
        dfy: join(".dfy"),
        r#gen: join(".dfy.gen"),
        base: join(".dfy.base"),
        merged: join(".dfy.merged"),
    }
}

fn read_if_exists(root: &Path, rel_path: &str) -> Option<String> {
    let abs = root.join(rel_path);
    if abs.is_file() {
        fs::read_to_string(abs).ok()
    } else {
        None
    }
}

pub fn validate_dfy_merge_artifacts(root: &Path, ts_files: &[String]) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();

    for ts_file in ts_files {
        let artifacts = dfy_artifact_paths(ts_file);

        for rel_path in [&artifacts.dfy, &artifacts.merged] {
            if let Some(content) = read_if_exists(root, rel_path)
                && has_merge_conflict_markers(&content)
            {
                issues.push(VerifyIssue {
                        severity: IssueSeverity::Critical,
                        phase: "proofs".to_string(),
                        requirement_id: None,
                        message: "Dafny proof file contains unresolved git merge conflict markers — resolve manually, then rerun `forall sync` or `lsc regen`.".to_string(),
                        file: Some(rel_path.to_string()),
                        counterexample: None,
                        pbt: None,
                        proof_detail: None,
                    });
            }
        }

        if read_if_exists(root, &artifacts.merged).is_some() {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: "Stale .dfy.merged artifact from a failed lsc regen — resolve conflicts in the .dfy file, delete the .merged file, then rerun `forall sync`.".to_string(),
                file: Some(artifacts.merged.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }

        if read_if_exists(root, &artifacts.base).is_some() {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "proofs".to_string(),
                requirement_id: None,
                message: "Incomplete lsc regen (.dfy.base present) — rerun `forall sync` or `lsc regen` to finish merging generated Dafny.".to_string(),
                file: Some(artifacts.base.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }
    }

    issues
}

pub fn cleanup_stale_dfy_merge_artifacts(root: &Path, rel_ts_file: &str) {
    let merged = root.join(dfy_artifact_paths(rel_ts_file).merged);
    if merged.is_file() {
        let _ = fs::remove_file(merged);
    }
}

pub fn format_lsc_regen_failure(rel_file: &str, result: &LscRunResult) -> String {
    let detail = format!("{}{}", result.stderr, result.stdout)
        .trim()
        .to_string();
    if detail.to_uppercase().contains("CONFLICT:") || has_merge_conflict_markers(&detail) {
        let artifacts = dfy_artifact_paths(rel_file);
        return format!(
            "lsc regen merge conflict for {rel_file}. Edit {} to resolve, delete {} when done, then rerun sync.",
            artifacts.dfy, artifacts.merged
        );
    }
    if !detail.is_empty() {
        return format_diagnostic_output(&detail);
    }
    format!("lsc regen failed for {rel_file}")
}

pub fn lsc_regen_clean(root: &Path, rel_file: &str) -> LscRunResult {
    let result = lsc_regen(root, rel_file);
    if result.ok {
        cleanup_stale_dfy_merge_artifacts(root, rel_file);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_git_conflict_markers() {
        assert!(has_merge_conflict_markers(
            "<<<<<<< HEAD\nfoo\n=======\nbar\n>>>>>>> gen"
        ));
        assert!(!has_merge_conflict_markers("const x := 1\n"));
    }

    #[test]
    fn maps_ts_files_to_sibling_dfy_artifacts() {
        assert_eq!(
            dfy_artifact_paths("src/login/password-policy.ts"),
            DfyArtifactPaths {
                dfy: "src/login/password-policy.dfy".to_string(),
                r#gen: "src/login/password-policy.dfy.gen".to_string(),
                base: "src/login/password-policy.dfy.base".to_string(),
                merged: "src/login/password-policy.dfy.merged".to_string(),
            }
        );
    }

    #[test]
    fn flags_stale_dfy_merged_files() {
        let root = tempdir().unwrap();
        let ts_file = "src/policy.ts";
        let merged = dfy_artifact_paths(ts_file).merged;
        let merged_path = root.path().join(&merged);
        fs::create_dir_all(merged_path.parent().unwrap()).unwrap();
        fs::write(
            &merged_path,
            "<<<<<<< HEAD\nold\n=======\nnew\n>>>>>>> gen\n",
        )
        .unwrap();

        let issues = validate_dfy_merge_artifacts(root.path(), &[ts_file.to_string()]);
        assert!(issues.iter().any(|issue| {
            issue.severity == IssueSeverity::Critical
                && issue.file.as_deref() == Some(merged.as_str())
                && issue.message.contains("Stale .dfy.merged")
        }));
    }

    #[test]
    fn removes_dfy_merged_after_successful_regen() {
        let root = tempdir().unwrap();
        let ts_file = "src/policy.ts";
        let merged = root.path().join(dfy_artifact_paths(ts_file).merged);
        fs::create_dir_all(merged.parent().unwrap()).unwrap();
        fs::write(&merged, "stale merge output").unwrap();

        cleanup_stale_dfy_merge_artifacts(root.path(), ts_file);
        assert!(!merged.exists());
    }
}
