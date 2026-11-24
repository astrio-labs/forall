use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::hooks::gate_error::{GateErrorCode, serialize_pre_tool_use_block};
use crate::hooks::paths::{
    ToolInputCarrier, extract_apply_patch_paths, extract_command_from_tool_input,
    extract_patch_text, normalize_project_relative,
};

static MAPPING_FILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|/)(?:mapping\.delta\.yaml|verify/mapping\.yaml|verified/mapping\.yaml)$")
        .unwrap_or_else(|err| panic!("invalid mapping file regex: {err}"))
});

fn is_mapping_file(rel: &str) -> bool {
    MAPPING_FILE.is_match(rel.trim_start_matches("./"))
}

fn mapping_path_on_disk(root: &Path, rel: &str) -> PathBuf {
    root.join(rel)
}

fn verified_requirement_ids(content: &str) -> HashSet<String> {
    let Ok(mapping) = serde_yaml::from_str::<crate::mapping::schema::Mapping>(content) else {
        return HashSet::new();
    };
    mapping
        .requirements
        .into_iter()
        .filter(|req| req.verified)
        .map(|req| req.id)
        .collect()
}

fn patch_downgrades_verified(patch: &str, current: &str) -> Vec<String> {
    let verified_ids = verified_requirement_ids(current);
    if verified_ids.is_empty() {
        return Vec::new();
    }

    let bulk_downgrade = (patch.contains("-    verified: true")
        || patch.contains("-  verified: true"))
        && (patch.contains("+    verified: false") || patch.contains("+  verified: false"));
    if bulk_downgrade {
        return verified_ids.into_iter().collect();
    }

    verified_ids
        .into_iter()
        .filter(|id| {
            patch.contains(&format!("id: {id}"))
                && (patch.contains("+    verified: false") || patch.contains("+  verified: false"))
        })
        .collect()
}

fn command_targets_mapping_downgrade(command: &str) -> bool {
    let touches_mapping = command.contains("mapping.delta.yaml")
        || command.contains("verify/mapping.yaml")
        || command.contains("verified/mapping.yaml");
    if !touches_mapping {
        return false;
    }

    if command.contains("replace('true','false')")
        || command.contains("replace(\"true\",\"false\")")
        || command.contains("verified: true") && command.contains("verified: false")
    {
        return true;
    }

    touches_mapping
        && command.contains("write_text")
        && command.contains("verified")
        && command.contains("false")
}

fn author_upsert_downgrades_verified(carrier: &ToolInputCarrier, current: &str) -> Vec<String> {
    let verified_ids = verified_requirement_ids(current);
    if verified_ids.is_empty() {
        return Vec::new();
    }
    [&carrier.tool_input, &carrier.tool_input_snake]
        .into_iter()
        .flatten()
        .find_map(|input| input.get("requirements").and_then(|value| value.as_array()))
        .into_iter()
        .flatten()
        .filter(|requirement| {
            requirement
                .get("verified")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        })
        .filter_map(|requirement| requirement.get("id").and_then(|value| value.as_str()))
        .filter(|id| verified_ids.contains(*id))
        .map(str::to_string)
        .collect()
}

/// Block edits that downgrade `verified: true` requirements to pass `forall check`.
pub fn check_verified_downgrade_block(
    root: &Path,
    tool_name: &str,
    carrier: &ToolInputCarrier,
) -> Option<String> {
    if std::env::var("FORALL_ALLOW_VERIFIED_DOWNGRADE")
        .ok()
        .as_deref()
        == Some("1")
    {
        return None;
    }

    let name = tool_name.to_lowercase();

    if name == "author.upsert_requirements" {
        let rel = ".forall/verify/mapping.yaml";
        if let Ok(current) = fs::read_to_string(mapping_path_on_disk(root, rel)) {
            let downgraded = author_upsert_downgrades_verified(carrier, &current);
            if !downgraded.is_empty() {
                return Some(block_payload(
                    &format!(
                        "Forall: cannot downgrade verified requirements ({}) in {rel}.",
                        downgraded.join(", ")
                    ),
                    "Fix the failing proof or scenario issue instead of setting verified: false.",
                    vec![rel.to_string()],
                ));
            }
        }
    }

    if Regex::new(r"(?i)apply_patch|applypatch|patch")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(&name)
    {
        let patch = extract_patch_text(carrier);
        for raw in extract_apply_patch_paths(&patch) {
            let rel = normalize_project_relative(root, &raw);
            if !is_mapping_file(&rel) {
                continue;
            }
            if let Ok(current) = fs::read_to_string(mapping_path_on_disk(root, &rel)) {
                let downgraded = patch_downgrades_verified(&patch, &current);
                if !downgraded.is_empty() {
                    return Some(block_payload(
                        &format!(
                            "Forall: cannot downgrade verified requirements ({}) in {rel} to get a green check.",
                            downgraded.join(", ")
                        ),
                        "Fix the failing proof or scenario issue instead of setting verified: false. Run `forall check --change <name>` and resolve CRITICAL issues, or ask the user to accept spec-tracked scope explicitly.",
                        vec![rel],
                    ));
                }
            }
        }
    }

    if Regex::new(r"(?i)exec_command|shell|bash|zsh")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(&name)
        && let Some(cmd) = extract_command_from_tool_input(carrier)
        && command_targets_mapping_downgrade(&cmd)
    {
        return Some(block_payload(
            "Forall: shell command appears to downgrade verified requirements in mapping.",
            "Do not rewrite mapping.yaml or mapping.delta.yaml to set verified: false after proofs or scenarios fail. Fix the underlying issue or ask the user to accept spec-tracked scope.",
            Vec::new(),
        ));
    }

    // The obligation ledger is the trusted evidence record — only
    // `forall check` writes it. Any direct edit, up or down, is forgery of
    // the artifact the citation surface renders from.
    if edits_ledger_artifact(&name, carrier) {
        return Some(block_payload(
            "Forall: the verify ledger is written only by `forall check`; direct edits are not allowed.",
            "Run `forall check` to regenerate evidence instead of editing *-ledger.json.",
            Vec::new(),
        ));
    }

    None
}

/// Whether a tool call writes a `*-ledger.json`: patch and write-shaped
/// tools by path inspection, shell tools by a write-shaped command that
/// names one. Reads stay allowed — the ledger is meant to be consumed.
///
/// This detector is deliberately defense-in-depth, not the security
/// boundary: no pattern list can enumerate every write form. The fail-closed
/// layer is the ledger's integrity seal — `read_ledger` discards any file
/// whose seal does not verify, so an edit that slips past this gate yields
/// no evidence rather than forged evidence.
fn edits_ledger_artifact(name: &str, carrier: &ToolInputCarrier) -> bool {
    let is_patch_or_write =
        Regex::new(r"(?i)apply_patch|applypatch|patch|write|edit|create_file|str_replace")
            .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
            .is_match(name);
    if is_patch_or_write {
        let patch = extract_patch_text(carrier);
        if extract_apply_patch_paths(&patch)
            .iter()
            .any(|path| path.contains("-ledger.json"))
        {
            return true;
        }
        // Write-shaped tools that carry a path field rather than patch text.
        return [&carrier.tool_input, &carrier.tool_input_snake]
            .into_iter()
            .flatten()
            .any(|input| {
                ["path", "file_path", "filename", "file"].iter().any(|key| {
                    input
                        .get(*key)
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|path| path.contains("-ledger.json"))
                })
            });
    }
    let is_shell = Regex::new(r"(?i)exec_command|shell|bash|zsh")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(name);
    if !is_shell {
        return false;
    }
    let Some(cmd) = extract_command_from_tool_input(carrier) else {
        return false;
    };
    cmd.contains("-ledger.json")
        && (cmd.contains('>')
            || cmd.contains("write_text")
            || cmd.contains("sed -i")
            || cmd.contains(" tee ")
            || cmd.contains("mv ")
            || cmd.contains("cp ")
            || cmd.contains("dd ")
            || cmd.contains("rsync")
            || cmd.contains("open("))
}

fn block_payload(message: &str, remediation: &str, paths: Vec<String>) -> String {
    serialize_pre_tool_use_block(
        GateErrorCode::VerifiedDowngradeBlocked,
        message,
        remediation,
        if paths.is_empty() { None } else { Some(paths) },
        None,
        None,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_patch_downgrade_of_verified_requirement() {
        let current = r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: "Clamp bounds"
    verified: true
"#;
        let patch = "*** Update File: .forall/workflow/changes/add-clamp/mapping.delta.yaml\n@@\n-    verified: true\n+    verified: false\n";
        let downgraded = patch_downgrades_verified(patch, current);
        assert!(downgraded.contains(&"clamp-bounds".to_string()));
    }

    #[test]
    fn detects_python_bulk_downgrade_command() {
        let cmd = r#"python3 - <<'PY'
p=Path('.forall/workflow/changes/x/mapping.delta.yaml')
out.append(line.replace('true','false'))
PY"#;
        assert!(command_targets_mapping_downgrade(cmd));
    }

    #[test]
    fn allows_adding_new_spec_tracked_requirement() {
        let current = r#"version: 1
requirements:
  - id: ui-copy
    capability: ui
    requirement: "UI copy"
    verified: false
"#;
        let patch = "*** Add File: mapping.delta.yaml\n+  - id: new-req\n+    verified: false\n";
        assert!(patch_downgrades_verified(patch, current).is_empty());
    }

    #[test]
    fn detects_native_author_upsert_downgrade() {
        let current = r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: "Clamp bounds"
    verified: true
"#;
        let carrier = ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "mode": "apply",
                "requirements": [{
                    "id": "clamp-bounds",
                    "capability": "math",
                    "requirement": "Clamp bounds",
                    "verified": false
                }]
            })),
            tool_input_snake: None,
        };
        assert_eq!(
            author_upsert_downgrades_verified(&carrier, current),
            vec!["clamp-bounds".to_string()]
        );
    }
}
