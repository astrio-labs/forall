use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::{FORALL_DIR, ensure_dir};

const EVIDENCE_REL: &str = ".session/browser-evidence.json";
const MAX_SESSIONS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrowserEvidenceStore {
    pub sessions: BTreeMap<String, BrowserSessionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSessionRecord {
    pub tools: Vec<String>,
    pub updated_at: String,
}

fn evidence_path(root: &Path) -> PathBuf {
    root.join(FORALL_DIR).join(EVIDENCE_REL)
}

fn empty_store() -> BrowserEvidenceStore {
    BrowserEvidenceStore::default()
}

fn read_store(root: &Path) -> BrowserEvidenceStore {
    let file = evidence_path(root);
    if !file.is_file() {
        return empty_store();
    }
    fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(empty_store)
}

fn write_store(root: &Path, store: &BrowserEvidenceStore) -> std::io::Result<()> {
    let file = evidence_path(root);
    if let Some(parent) = file.parent() {
        ensure_dir(parent)?;
    }
    let mut entries: Vec<_> = store.sessions.iter().collect();
    entries.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));
    let trimmed: BTreeMap<_, _> = entries
        .into_iter()
        .take(MAX_SESSIONS)
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let out = BrowserEvidenceStore { sessions: trimmed };
    fs::write(file, format!("{}\n", serde_json::to_string_pretty(&out)?))
}

pub fn is_browser_evidence_tool(tool_name: &str) -> bool {
    let name = tool_name.trim();
    if name.is_empty() {
        return false;
    }
    let lower = name.to_lowercase();

    if regex::Regex::new(
        r"(?i)^browser_(navigate|snapshot|click|take_screenshot|fill|select|scroll|hover|type|press_key|lock|tabs|cdp)",
    )
    .unwrap_or_else(|err| panic!("invalid browser tool regex: {err}"))
    .is_match(&lower)
    {
        return true;
    }
    if regex::Regex::new(r"(?i)mcp__[^_]*browser[^_]*__browser_")
        .unwrap_or_else(|err| panic!("invalid browser tool regex: {err}"))
        .is_match(&lower)
    {
        return true;
    }
    if lower.contains("cursor-ide-browser") && lower.contains("browser_") {
        return true;
    }
    if regex::Regex::new(r"(?i)mcp__playwright__")
        .unwrap_or_else(|err| panic!("invalid browser tool regex: {err}"))
        .is_match(&lower)
    {
        return true;
    }
    if regex::Regex::new(r"(?i)^mcp__cursor-app-control__open_resource$")
        .unwrap_or_else(|err| panic!("invalid browser tool regex: {err}"))
        .is_match(&lower)
    {
        return true;
    }

    false
}

pub fn record_browser_evidence(root: &Path, session_id: &str, tool_name: &str) {
    if !is_browser_evidence_tool(tool_name) {
        return;
    }
    let mut store = read_store(root);
    let mut existing = store
        .sessions
        .remove(session_id)
        .unwrap_or(BrowserSessionRecord {
            tools: Vec::new(),
            updated_at: String::new(),
        });
    if !existing.tools.iter().any(|t| t == tool_name) {
        existing.tools.push(tool_name.to_string());
    }
    existing.updated_at = chrono_lite_now();
    store.sessions.insert(session_id.to_string(), existing);
    let _ = write_store(root, &store);
}

pub fn session_has_browser_evidence(root: &Path, session_id: &str) -> bool {
    read_store(root)
        .sessions
        .get(session_id)
        .is_some_and(|s| !s.tools.is_empty())
}

pub fn transcript_has_browser_evidence(transcript_path: &str) -> bool {
    let path = Path::new(transcript_path);
    if !path.is_file() {
        return false;
    }
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(row_type) = row.get("type").and_then(|v| v.as_str()) else {
            continue;
        };
        if row_type == "event_msg" {
            let payload = row.get("payload").cloned().unwrap_or_default();
            if payload.get("type").and_then(|v| v.as_str()) == Some("mcp_tool_call_begin")
                || payload.get("type").and_then(|v| v.as_str()) == Some("mcp_tool_call_end")
            {
                let invocation = payload.get("invocation").cloned().unwrap_or_default();
                let server = invocation
                    .get("server")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let tool = invocation
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if is_browser_evidence_tool(&format!("{server}/{tool}"))
                    || is_browser_evidence_tool(tool)
                {
                    return true;
                }
            }
        }
        if row_type == "response_item" {
            let payload = row.get("payload").cloned().unwrap_or_default();
            if payload.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let namespace = payload
                    .get("namespace")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let qualified = if namespace.is_empty() {
                    name.to_string()
                } else {
                    format!("{namespace}__{name}")
                };
                if is_browser_evidence_tool(&qualified) || is_browser_evidence_tool(name) {
                    return true;
                }
            }
        }
    }
    false
}

use chrono::Utc;

fn chrono_lite_now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn recognizes_browser_tools() {
        assert!(is_browser_evidence_tool("browser_navigate"));
        assert!(is_browser_evidence_tool(
            "mcp__cursor-ide-browser__browser_snapshot"
        ));
        assert!(is_browser_evidence_tool(
            "mcp__playwright__browser_snapshot"
        ));
        assert!(!is_browser_evidence_tool("exec_command"));
    }

    #[test]
    fn records_session_evidence() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        ensure_dir(&root.join(FORALL_DIR)).unwrap();
        record_browser_evidence(root, "sess-1", "browser_snapshot");
        assert!(session_has_browser_evidence(root, "sess-1"));
    }

    #[test]
    fn detects_rollout_browser_calls() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("rollout.jsonl");
        fs::write(
            &file,
            serde_json::json!({
                "type": "event_msg",
                "payload": {
                    "type": "mcp_tool_call_begin",
                    "invocation": {
                        "server": "cursor-ide-browser",
                        "tool": "browser_navigate"
                    }
                }
            })
            .to_string()
                + "\n",
        )
        .unwrap();
        assert!(transcript_has_browser_evidence(&file.display().to_string()));
    }
}
