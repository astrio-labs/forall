use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context;
use serde::Deserialize;
use tokio::process::Command;

const CONFIG_REL: &str = ".forall/verified/config.yaml";
const LEGACY_CONFIG_REL: &str = "forall/verified/config.yaml";

/// Subcommands handled by the native Rust workflow engine (`agent/workflow`).
const WORKFLOW_COMMANDS: &[&str] = &[
    "bootstrap",
    "init",
    "ensure",
    "ensure-rust",
    "ensure-java",
    "ensure-c",
    "ensure-verus",
    "ensure-lsc",
    "ensure-openjml",
    "ensure-framac",
    "get-workflow-status",
    "propose",
    "list",
    "status",
    "instructions",
    "sync",
    "check",
    "assurance",
    "export",
    "mapping",
    "hook",
    "daemon",
    "skills",
    "hooks",
];

#[derive(Debug, Deserialize)]
struct VerifiedConfig {
    cli_js: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForallCliCommand {
    pub program: String,
    pub args: Vec<String>,
}

/// True when argv should be handled by the workflow engine instead of the agent CLI.
pub fn should_delegate_to_workflow_cli(args: &[impl AsRef<str>]) -> bool {
    let Some(sub) = first_subcommand(args) else {
        return false;
    };
    if WORKFLOW_COMMANDS.contains(&sub.as_str()) {
        return true;
    }
    if sub == "archive" {
        return is_workflow_archive(args);
    }
    false
}

/// Resolve the `forall` executable for subprocess calls (specs.* tools).
pub fn resolve_forall_cli(project_root: &Path) -> anyhow::Result<ForallCliCommand> {
    resolve_forall_cli_for_cwd(project_root, std::env::current_exe().ok().as_deref())
}

/// Resolve workflow CLI from env, project config, or the running binary.
pub fn resolve_forall_cli_for_cwd(
    cwd: &Path,
    exe: Option<&Path>,
) -> anyhow::Result<ForallCliCommand> {
    if let Ok(raw) = std::env::var("FORALL_CLI") {
        let parts = shell_words(&raw);
        if parts.is_empty() {
            anyhow::bail!("FORALL_CLI is empty");
        }
        let program = parts[0].clone();
        let args = parts[1..].to_vec();
        return Ok(ForallCliCommand { program, args });
    }

    if let Ok(path) = std::env::var("FORALL_CLI_JS") {
        return Ok(cli_entry_command(&path));
    }

    if let Some(root) = find_project_root(cwd)
        && let Some(path) = read_cli_js_from_config(&root)
    {
        return Ok(cli_entry_command(&path));
    }

    let program = exe
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "forall".to_string());
    Ok(ForallCliCommand {
        program,
        args: Vec::new(),
    })
}

fn cli_entry_command(path: &str) -> ForallCliCommand {
    if path.ends_with(".js") {
        ForallCliCommand {
            program: "node".to_string(),
            args: vec![path.to_string()],
        }
    } else {
        ForallCliCommand {
            program: path.to_string(),
            args: Vec::new(),
        }
    }
}

fn read_cli_js_from_config(project_root: &Path) -> Option<String> {
    for rel in [CONFIG_REL, LEGACY_CONFIG_REL] {
        let path = project_root.join(rel);
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(parsed) = serde_yaml::from_str::<VerifiedConfig>(&raw)
            && let Some(entry) = parsed.cli_js.filter(|s| !s.trim().is_empty())
        {
            return Some(entry);
        }
    }
    None
}

/// Resolve the project root, matching `forall-workflow`'s marker logic so that
/// native `specs.*` tools recognize projects created by `forall init` (Option B
/// layout: `.forall/verify/mapping.yaml`) as well as older legacy layouts
/// (`.forall/verified/mapping.yaml`, `forall/verified/mapping.yaml`). Also
/// accepts `.forall/markers.toml`, written by `forall init` but not checked by
/// `forall_workflow::find_project_root`.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    if let Some(root) = forall_workflow::find_project_root(start) {
        return Some(root);
    }

    let mut dir = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if forall_workflow::paths::agent_markers_path(&dir).is_file() {
            return Some(dir);
        }
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        dir = parent.to_path_buf();
    }
}

pub async fn run_forall_cli(cwd: &Path, cli_args: &[&str]) -> anyhow::Result<(i32, String)> {
    let root = find_project_root(cwd)
        .with_context(|| format!("no Forall project under {}", cwd.display()))?;
    let cmd = resolve_forall_cli(&root)?;

    let output = Command::new(&cmd.program)
        .args(&cmd.args)
        .args(cli_args)
        .current_dir(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("failed to spawn {} {:?}", cmd.program, cmd.args))?;

    let code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = stdout.into_owned();
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    Ok((code, combined))
}

fn first_subcommand(args: &[impl AsRef<str>]) -> Option<String> {
    let mut i = 1;
    while i < args.len() {
        let a = args[i].as_ref();
        if a == "--" {
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        return Some(a.to_string());
    }
    None
}

/// Distinguish spec-change `archive` from session `archive`.
fn is_workflow_archive(args: &[impl AsRef<str>]) -> bool {
    for arg in args.iter().skip(1).map(AsRef::as_ref) {
        if arg == "--skip-check" || arg == "--skip-specs" {
            return true;
        }
        if arg == "--remote" || arg.starts_with("--config-profile") {
            return false;
        }
    }

    if let Some(name) = args.get(2)
        && let Ok(cwd) = std::env::current_dir()
        && let Some(root) = find_project_root(&cwd)
    {
        let change_dir = root.join(".forall/changes").join(name.as_ref());
        if change_dir.is_dir() {
            return true;
        }
    }

    false
}

fn shell_words(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_subcommands_delegate() {
        let args = ["forall", "get-workflow-status", "--root", "."];
        assert!(should_delegate_to_workflow_cli(&args));
    }

    #[test]
    fn hook_subcommand_delegates() {
        let args = ["forall", "hook", "pre-tool-use"];
        assert!(should_delegate_to_workflow_cli(&args));
    }

    #[test]
    fn agent_subcommands_do_not_delegate() {
        let args = ["forall", "exec", "hello"];
        assert!(!should_delegate_to_workflow_cli(&args));
    }

    #[test]
    fn workflow_archive_flags_delegate() {
        let args = ["forall", "archive", "my-change", "--skip-check"];
        assert!(should_delegate_to_workflow_cli(&args));
    }

    #[test]
    fn ensure_toolchain_subcommands_delegate() {
        for sub in [
            "ensure-rust",
            "ensure-java",
            "ensure-c",
            "ensure-verus",
            "ensure-lsc",
            "ensure-openjml",
            "ensure-framac",
        ] {
            let args = ["forall", sub, "--check-only"];
            assert!(
                should_delegate_to_workflow_cli(&args),
                "{sub} should delegate to workflow"
            );
        }
    }

    #[test]
    fn session_archive_does_not_delegate() {
        let args = ["forall", "archive", "session-uuid", "--remote", "prod"];
        assert!(!should_delegate_to_workflow_cli(&args));
    }

    #[test]
    fn js_cli_entry_uses_node() {
        let cmd = cli_entry_command("/tmp/forall.js");
        assert_eq!(cmd.program, "node");
        assert_eq!(cmd.args, vec!["/tmp/forall.js"]);
    }

    #[test]
    fn native_cli_entry_uses_binary() {
        let cmd = cli_entry_command("/usr/local/bin/forall");
        assert_eq!(cmd.program, "/usr/local/bin/forall");
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn find_project_root_detects_option_b_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".forall/verify")).unwrap();
        std::fs::write(
            dir.path().join(".forall/verify/mapping.yaml"),
            "version: 1\n",
        )
        .unwrap();

        let nested = dir.path().join("src/nested");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            find_project_root(&nested).map(|p| p.canonicalize().ok()),
            Some(dir.path().canonicalize().ok())
        );
    }

    #[test]
    fn find_project_root_detects_dot_forall_legacy_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".forall/verified")).unwrap();
        std::fs::write(
            dir.path().join(".forall/verified/mapping.yaml"),
            "version: 1\n",
        )
        .unwrap();

        assert_eq!(
            find_project_root(dir.path()).map(|p| p.canonicalize().ok()),
            Some(dir.path().canonicalize().ok())
        );
    }

    #[test]
    fn find_project_root_detects_pre_dot_legacy_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("forall/verified")).unwrap();
        std::fs::write(
            dir.path().join("forall/verified/mapping.yaml"),
            "version: 1\n",
        )
        .unwrap();

        assert_eq!(
            find_project_root(dir.path()).map(|p| p.canonicalize().ok()),
            Some(dir.path().canonicalize().ok())
        );
    }

    #[test]
    fn find_project_root_detects_agent_markers_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".forall")).unwrap();
        std::fs::write(dir.path().join(".forall/markers.toml"), "").unwrap();

        assert_eq!(
            find_project_root(dir.path()).map(|p| p.canonicalize().ok()),
            Some(dir.path().canonicalize().ok())
        );
    }

    #[test]
    fn find_project_root_returns_none_without_markers() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_project_root(dir.path()), None);
    }
}
