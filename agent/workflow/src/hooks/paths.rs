use crate::paths::{FORALL_DIR, LEGACY_FORALL_DIR};

pub const PRE_TOOL_USE_MATCHER: &str = "apply_patch|write|edit|Write|Edit|ApplyPatch|exec_command|ExecCommand|shell|author\\.(init|upsert_requirements|scaffold_contracts|scaffold_property)";

static CODE_EXTENSIONS: &str =
    r"\.(ts|tsx|js|jsx|mjs|cjs|py|rs|go|java|html|htm|css|vue|svelte|dart|kt|swift)$";

static ROOT_CODE_DIRS: &str = r"^(src|lib|app|packages|crates|agent|cli|mastermind|public|www|frontend|backend|server|api|web|ui)/";

use regex::Regex;
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

static CODE_EXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(CODE_EXTENSIONS).unwrap_or_else(|err| panic!("invalid code extension regex: {err}"))
});
static ROOT_CODE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(ROOT_CODE_DIRS).unwrap_or_else(|err| panic!("invalid root code dir regex: {err}"))
});
static ROOT_DIR_IN_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|/)(src|lib|app|mastermind|packages|crates|agent|cli|public|www|frontend|backend|server|api|web|ui)/")
        .unwrap_or_else(|err| panic!("invalid root dir path regex: {err}"))
});
static PATCH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\*\*\* (?:Update|Add|Delete|Move) File:\s+(.+?)\s*$")
        .unwrap_or_else(|err| panic!("invalid patch header regex: {err}"))
});

/// Normalize absolute or cwd-prefixed paths to project-relative posix paths.
pub fn normalize_project_relative(root: &Path, raw_path: &str) -> String {
    let mut p = raw_path
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string();
    if p.is_empty() || p.starts_with("/dev/") {
        return String::new();
    }

    if p.starts_with("./") {
        p = p[2..].to_string();
    }
    let path_obj = Path::new(&p);
    if path_obj.is_absolute()
        && let Ok(rel) = path_obj.strip_prefix(root)
    {
        p = rel.display().to_string();
    }
    p = p.replace('\\', "/");

    let forall_slash = format!("{FORALL_DIR}/");
    if let Some(idx) = p.find(&forall_slash) {
        if idx > 0 {
            p = p[idx..].to_string();
        }
    } else {
        let legacy_slash = format!("{LEGACY_FORALL_DIR}/");
        if let Some(idx) = p.find(&legacy_slash) {
            if idx > 0 {
                p = p[idx..].to_string();
            }
        } else if let Some(m) = ROOT_DIR_IN_PATH.find(&p) {
            let start = if m.start() == 0 { 0 } else { m.start() + 1 };
            p = p[start..].to_string();
        }
    }

    if p.starts_with("../") {
        return String::new();
    }
    p
}

fn is_under_forall_dir(rel: &str, suffix: &str) -> bool {
    rel.starts_with(&format!("{FORALL_DIR}/{suffix}"))
        || rel.starts_with(&format!("{LEGACY_FORALL_DIR}/{suffix}"))
}

pub fn is_planning_path(rel: &str) -> bool {
    is_under_forall_dir(rel, "workflow/changes/")
        || is_under_forall_dir(rel, "changes/")
        || is_under_forall_dir(rel, "specs/")
}

pub fn is_exempt_path(rel: &str) -> bool {
    is_under_forall_dir(rel, "verify/cache/reports/")
        || is_under_forall_dir(rel, "verified/reports/")
        || is_under_forall_dir(rel, "verify/cache/intent/")
        || is_under_forall_dir(rel, "verified/claimcheck/")
        || is_under_forall_dir(rel, "verify/cache/waivers/")
        || is_under_forall_dir(rel, "session/waivers/")
        || rel == format!("{FORALL_DIR}/workflow/config.yaml")
        || rel == format!("{FORALL_DIR}/config.yaml")
        || rel == format!("{LEGACY_FORALL_DIR}/config.yaml")
        || rel == format!("{FORALL_DIR}/hooks.json")
        || rel == format!("{FORALL_DIR}/agent/hooks.json")
        || rel == format!("{LEGACY_FORALL_DIR}/hooks.json")
        || rel.starts_with(&format!("{FORALL_DIR}/hooks/"))
        || rel.starts_with(&format!("{LEGACY_FORALL_DIR}/hooks/"))
        || rel == format!("{FORALL_DIR}/workflow/active")
        || rel == format!("{FORALL_DIR}/.active-change")
        || rel == format!("{LEGACY_FORALL_DIR}/.active-change")
        || rel == format!("{FORALL_DIR}/markers.toml")
        || rel == format!("{FORALL_DIR}/agent/markers.toml")
        || rel == format!("{FORALL_DIR}/config.toml")
        || rel == format!("{FORALL_DIR}/agent/AGENTS.md")
        || rel == format!("{FORALL_DIR}/AGENTS.md")
        || rel == format!("{LEGACY_FORALL_DIR}/AGENTS.md")
        || rel == format!("{FORALL_DIR}/verify/mapping.yaml")
        || rel == format!("{FORALL_DIR}/verified/mapping.yaml")
        || rel == format!("{FORALL_DIR}/verify/adapters.yaml")
        || rel == format!("{FORALL_DIR}/verified/config.yaml")
        || rel == "AGENTS.md"
        || rel.starts_with(".codex/")
        || rel.starts_with(".cursor/")
        || rel.starts_with(".git/")
}

/// Writable application code (not workflow planning artifacts).
pub fn is_application_code_path(rel: &str) -> bool {
    if rel.is_empty() || is_planning_path(rel) || is_exempt_path(rel) {
        return false;
    }
    if rel.starts_with(&format!("{FORALL_DIR}/scenarios/")) && rel.ends_with(".property.ts") {
        return true;
    }
    if rel.starts_with(&format!("{FORALL_DIR}/"))
        || rel.starts_with(&format!("{LEGACY_FORALL_DIR}/"))
    {
        return false;
    }
    if CODE_EXT_RE.is_match(rel) {
        return true;
    }
    ROOT_CODE_RE.is_match(rel)
}

pub fn extract_apply_patch_paths(patch: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in patch.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(target) = PATCH_HEADER.captures(line).and_then(|caps| caps.get(1)) {
            paths.push(target.as_str().to_string());
        }
    }
    paths
}

/// Detect shell redirection / heredoc targets that create or overwrite files.
pub fn extract_shell_write_paths(command: &str) -> Vec<String> {
    let patterns: Vec<Regex> = [
        r"\bcat\s+>\s*([^\s|;&]+)\s*<<",
        r"\btee\s+([^\s|;&]+)",
        r"\b(?:cp|mv|install)\s+[^\s|;&]+\s+([^\s|;&]+)",
        r"\btouch\s+([^\s|;&]+)",
        r"(?:^|[|;&\s])>>?\s*([^\s|;&]+)",
        r##"\bsed\s+-i(?:\.\w+)?(?:\s+(?:''|""))?\s+([^\s|;&]+)"##,
        r##"\bgit\s+(?:checkout|restore)\b(?:\s+[^\s|;&]+)*\s+--(?:\s+[^\s|;&]+)*\s+([^\s|;&]+)"##,
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap_or_else(|err| panic!("invalid shell write regex {p:?}: {err}")))
    .collect();

    let mut paths = HashSet::new();
    for pattern in &patterns {
        for caps in pattern.captures_iter(command) {
            let target = caps
                .get(1)
                .map(|m| m.as_str().trim_matches(|c| c == '"' || c == '\''))
                .unwrap_or("");
            if !target.is_empty()
                && !target.starts_with("/dev/")
                && target != "&1"
                && target != "&2"
            {
                paths.insert(target.to_string());
            }
        }
    }
    paths.into_iter().collect()
}

#[derive(Debug, Clone, Default)]
pub struct ToolInputCarrier {
    pub tool_input: Option<serde_json::Value>,
    pub tool_input_snake: Option<serde_json::Value>,
}

pub(crate) fn extract_patch_text(input: &ToolInputCarrier) -> String {
    for blob in [&input.tool_input, &input.tool_input_snake] {
        let Some(blob) = blob else { continue };
        if let Some(s) = blob.as_str() {
            return s.to_string();
        }
        if let Some(obj) = blob.as_object() {
            for key in ["command", "patch", "input", "content", "text"] {
                if let Some(value) = obj.get(key).and_then(|v| v.as_str()) {
                    return value.to_string();
                }
            }
            return serde_json::to_string(blob).unwrap_or_default();
        }
    }
    String::new()
}

pub fn extract_command_from_tool_input(input: &ToolInputCarrier) -> Option<String> {
    for blob in [&input.tool_input, &input.tool_input_snake] {
        let Some(blob) = blob else { continue };
        if let Some(s) = blob.as_str() {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                return parsed
                    .get("cmd")
                    .or_else(|| parsed.get("command"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or_else(|| Some(s.to_string()));
            }
            return Some(s.to_string());
        }
        if let Some(obj) = blob.as_object() {
            return obj
                .get("cmd")
                .or_else(|| obj.get("command"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
        }
    }
    None
}

pub fn extract_structured_write_paths(input: &ToolInputCarrier) -> Vec<String> {
    let mut paths = HashSet::new();
    for blob in [&input.tool_input, &input.tool_input_snake] {
        let Some(blob) = blob else { continue };
        let Some(obj) = blob.as_object() else {
            continue;
        };
        for key in ["path", "file_path", "filePath", "relative_path"] {
            if let Some(value) = obj.get(key).and_then(|v| v.as_str()) {
                paths.insert(value.to_string());
            }
        }
        if let Some(values) = obj.get("paths").and_then(|value| value.as_array()) {
            paths.extend(
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string)),
            );
        }
    }
    paths.into_iter().collect()
}

pub fn is_pre_tool_use_gated_tool(tool_name: &str) -> bool {
    if tool_name.is_empty() {
        return true;
    }
    Regex::new(
        r"(?i)write|edit|apply_patch|patch|exec_command|shell|author\.(?:init|upsert_requirements|scaffold_contracts|scaffold_property)",
    )
        .unwrap_or_else(|err| panic!("invalid gated tool regex: {err}"))
        .is_match(tool_name)
}

pub fn extract_edit_paths_from_tool(
    root: &Path,
    tool_name: &str,
    input: &ToolInputCarrier,
) -> Vec<String> {
    let mut normalized = HashSet::new();
    let name = tool_name.to_lowercase();

    if name.starts_with("author.") {
        for raw in extract_structured_write_paths(input) {
            let rel = normalize_project_relative(root, &raw);
            if !rel.is_empty() {
                normalized.insert(rel);
            }
        }
        return normalized.into_iter().collect();
    }

    if Regex::new(r"(?i)apply_patch|applypatch|patch")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(&name)
    {
        for raw in extract_apply_patch_paths(&extract_patch_text(input)) {
            let rel = normalize_project_relative(root, &raw);
            if !rel.is_empty() {
                normalized.insert(rel);
            }
        }
        return normalized.into_iter().collect();
    }

    if Regex::new(r"(?i)exec_command|shell|bash|zsh")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(&name)
    {
        if let Some(cmd) = extract_command_from_tool_input(input) {
            for raw in extract_shell_write_paths(&cmd) {
                let rel = normalize_project_relative(root, &raw);
                if !rel.is_empty() {
                    normalized.insert(rel);
                }
            }
        }
        return normalized.into_iter().collect();
    }

    if Regex::new(r"(?i)write|edit")
        .unwrap_or_else(|err| panic!("invalid tool name regex: {err}"))
        .is_match(&name)
    {
        for raw in extract_structured_write_paths(input) {
            let rel = normalize_project_relative(root, &raw);
            if !rel.is_empty() {
                normalized.insert(rel);
            }
        }
    }

    normalized.into_iter().collect()
}

pub fn tool_input_from_json(value: serde_json::Value) -> ToolInputCarrier {
    ToolInputCarrier {
        tool_input: value.get("toolInput").cloned(),
        tool_input_snake: value
            .get("tool_input")
            .cloned()
            .or_else(|| value.get("toolInput").cloned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn root() -> PathBuf {
        PathBuf::from("/tmp/myproject")
    }

    #[test]
    fn normalize_strips_absolute_paths() {
        assert_eq!(
            normalize_project_relative(&root(), "/tmp/myproject/src/clamp.ts"),
            "src/clamp.ts"
        );
    }

    #[test]
    fn normalize_extracts_app_path_from_long_absolute() {
        assert_eq!(
            normalize_project_relative(&root(), "/var/folders/abc/myproject/mastermind/script.js"),
            "mastermind/script.js"
        );
    }

    #[test]
    fn normalize_ignores_paths_outside_project() {
        assert_eq!(normalize_project_relative(&root(), "../outside.ts"), "");
    }

    #[test]
    fn extract_shell_write_paths_cp_and_mv() {
        assert_eq!(
            extract_shell_write_paths("cp /tmp/a.ts src/out.ts"),
            vec!["src/out.ts".to_string()]
        );
        assert_eq!(
            extract_shell_write_paths("mv old.ts src/new.ts"),
            vec!["src/new.ts".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_append_redirect() {
        assert_eq!(
            extract_shell_write_paths("echo line >> src/log.ts"),
            vec!["src/log.ts".to_string()]
        );
    }

    #[test]
    fn application_code_includes_mastermind_and_src() {
        assert!(is_application_code_path("mastermind/script.js"));
        assert!(is_application_code_path("src/clamp.ts"));
    }

    #[test]
    fn application_code_excludes_forall_agents_md() {
        assert!(!is_application_code_path(".forall/agent/AGENTS.md"));
    }

    #[test]
    fn application_code_excludes_workflow_planning() {
        assert!(!is_application_code_path(
            ".forall/workflow/changes/foo/proposal.md"
        ));
        assert!(!is_application_code_path(".forall/changes/foo/proposal.md"));
        assert!(!is_application_code_path(".forall/specs/math/spec.md"));
    }

    #[test]
    fn application_code_includes_author_property_modules() {
        assert!(is_application_code_path(
            ".forall/scenarios/clamp-bounds.property.ts"
        ));
    }

    #[test]
    fn extract_patch_text_reads_command_field() {
        let carrier = ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "command": "*** Add File: src/foo.ts\n+line\n"
            })),
            tool_input_snake: None,
        };
        let patch = super::extract_patch_text(&carrier);
        assert!(patch.contains("src/foo.ts"));
    }

    #[test]
    fn extract_apply_patch_paths_parses_headers() {
        let patch = "*** Update File: src/foo.ts\n+line\n*** Add File: mastermind/bar.js\n";
        assert_eq!(
            extract_apply_patch_paths(patch),
            vec!["src/foo.ts".to_string(), "mastermind/bar.js".to_string()]
        );
    }

    #[test]
    fn extract_apply_patch_ignores_prose() {
        let prose = "We should edit src/foo.ts next.";
        assert!(extract_apply_patch_paths(prose).is_empty());
    }

    #[test]
    fn extract_shell_write_paths_heredoc() {
        let cmd = "cat > mastermind/script.js <<'EOF'\nconsole.log(1);\nEOF";
        assert_eq!(
            extract_shell_write_paths(cmd),
            vec!["mastermind/script.js".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_tee() {
        assert_eq!(
            extract_shell_write_paths("tee src/out.ts"),
            vec!["src/out.ts".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_touch() {
        assert_eq!(
            extract_shell_write_paths("touch src/x.ts"),
            vec!["src/x.ts".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_echo_redirect() {
        assert_eq!(
            extract_shell_write_paths("echo x > src/foo.ts"),
            vec!["src/foo.ts".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_sed() {
        assert_eq!(
            extract_shell_write_paths("sed -i '' src/foo.ts"),
            vec!["src/foo.ts".to_string()]
        );
        assert_eq!(
            extract_shell_write_paths("sed -i.bak src/foo.ts"),
            vec!["src/foo.ts".to_string()]
        );
    }

    #[test]
    fn extract_shell_write_paths_git_checkout() {
        assert_eq!(
            extract_shell_write_paths("git checkout -- src/foo.ts"),
            vec!["src/foo.ts".to_string()]
        );
        assert_eq!(
            extract_shell_write_paths("git restore --source=HEAD -- src/clamp.ts"),
            vec!["src/clamp.ts".to_string()]
        );
        assert_eq!(
            extract_shell_write_paths("git restore -- src/clamp.ts"),
            vec!["src/clamp.ts".to_string()]
        );
        assert!(extract_shell_write_paths("git status").is_empty());
        assert!(extract_shell_write_paths("git diff src/foo.ts").is_empty());
        assert!(extract_shell_write_paths("git checkout main").is_empty());
    }

    #[test]
    fn extract_edit_paths_gates_exec_command() {
        let input = ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "cmd": "cat > src/hack.ts <<'EOF'\nexport {}\nEOF"
            })),
            tool_input_snake: None,
        };
        assert_eq!(
            extract_edit_paths_from_tool(&root(), "exec_command", &input),
            vec!["src/hack.ts".to_string()]
        );
    }

    #[test]
    fn extract_edit_paths_empty_for_prose_patch() {
        let input = ToolInputCarrier {
            tool_input: Some(serde_json::json!(
                "Discuss changes under src/ without headers"
            )),
            tool_input_snake: None,
        };
        assert!(extract_edit_paths_from_tool(&root(), "apply_patch", &input).is_empty());
    }

    #[test]
    fn extract_edit_paths_reads_native_author_paths() {
        let input = ToolInputCarrier {
            tool_input: Some(serde_json::json!({
                "mode": "apply",
                "paths": ["src/clamp.ts"]
            })),
            tool_input_snake: None,
        };
        assert_eq!(
            extract_edit_paths_from_tool(&root(), "author.scaffold_contracts", &input),
            vec!["src/clamp.ts".to_string()]
        );
    }

    #[test]
    fn is_pre_tool_use_gated_tool_matches() {
        assert!(is_pre_tool_use_gated_tool("exec_command"));
        assert!(is_pre_tool_use_gated_tool("apply_patch"));
        assert!(is_pre_tool_use_gated_tool("author.scaffold_contracts"));
        assert!(is_pre_tool_use_gated_tool("author.scaffold_property"));
        assert!(!is_pre_tool_use_gated_tool("read_file"));
    }
}
