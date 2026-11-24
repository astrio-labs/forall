//! Workflow and verification diagnostics for `forall doctor`.
//!
//! These checks are read-only and safe to run outside an agent session. They
//! inspect project layout, hooks, CLI wiring, proof engine resolution, and
//! adapter configuration.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::active_change::{read_active_change_file, resolve_active_change};
use crate::agent_home::resolve_forall_home;
use crate::paths::{find_project_root, forall_mapping_path, hooks_json_read_path};
use crate::verify::adapters::{load_verified_config, verified_config_path};
use crate::verify::backends::{proof_engine_doctor_checks, verus};
use crate::workflow::paths::list_active_changes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowDoctorStatus {
    Ok,
    Warning,
    Fail,
}

impl WorkflowDoctorStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "warning",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDoctorCheck {
    pub id: String,
    pub status: WorkflowDoctorStatus,
    pub summary: String,
    pub details: Vec<String>,
    pub remediation: Option<String>,
}

impl WorkflowDoctorCheck {
    pub(crate) fn new(
        id: impl Into<String>,
        status: WorkflowDoctorStatus,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status,
            summary: summary.into(),
            details: Vec::new(),
            remediation: None,
        }
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    pub(crate) fn details(mut self, details: Vec<String>) -> Self {
        self.details.extend(details);
        self
    }

    pub(crate) fn remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

/// Run verified-project doctor checks for `cwd`.
pub fn run_verified_doctor(cwd: &Path) -> Vec<WorkflowDoctorCheck> {
    let Some(root) = find_project_root(cwd) else {
        return vec![
            WorkflowDoctorCheck::new(
                "verified.project",
                WorkflowDoctorStatus::Warning,
                "not inside a Forall project",
            )
            .detail(format!("cwd: {}", cwd.display()))
            .remediation("Run `forall init` in the project root, then rerun `forall doctor`."),
        ];
    };

    let mut checks = vec![
        project_check(&root),
        trust_check(&root),
        hooks_check(&root),
        cli_check(&root),
        adapters_check(&root),
        active_change_check(&root),
    ];
    let config = load_verified_config(&root);
    checks.extend(proof_engine_doctor_checks(&config));
    if let Some(cargo_check) = verus::doctor_cargo_metadata_check(&root) {
        checks.push(cargo_check);
    }
    checks
}

fn project_check(root: &Path) -> WorkflowDoctorCheck {
    let mapping = forall_mapping_path(root);
    let verified_config = verified_config_path(root);
    let details = vec![
        format!("project root: {}", root.display()),
        format!(
            "mapping: {}",
            if mapping.is_file() {
                "present".to_string()
            } else {
                "missing".to_string()
            }
        ),
        format!(
            "adapters config: {}",
            if verified_config.is_file() {
                "present".to_string()
            } else {
                "missing".to_string()
            }
        ),
    ];

    let status = if mapping.is_file() {
        WorkflowDoctorStatus::Ok
    } else {
        WorkflowDoctorStatus::Fail
    };

    let mut check = WorkflowDoctorCheck::new(
        "verified.project",
        status,
        if status == WorkflowDoctorStatus::Ok {
            "project layout looks good"
        } else {
            "project layout incomplete"
        },
    )
    .details(details);

    if status == WorkflowDoctorStatus::Fail {
        check = check.remediation("Run `forall init` or restore `.forall/verify/` files.");
    }
    check
}

fn trust_check(root: &Path) -> WorkflowDoctorCheck {
    let config_path = resolve_forall_home().join("config.toml");
    let mut details = vec![format!("FORALL_HOME config: {}", config_path.display())];

    if !config_path.is_file() {
        return WorkflowDoctorCheck::new(
            "verified.trust",
            WorkflowDoctorStatus::Warning,
            "project trust not recorded",
        )
        .details(details)
        .detail("config.toml not found in FORALL_HOME")
        .remediation("Run `forall init` in the project to trust the workspace.");
    }

    let content = match fs::read_to_string(&config_path) {
        Ok(raw) => raw,
        Err(err) => {
            return WorkflowDoctorCheck::new(
                "verified.trust",
                WorkflowDoctorStatus::Warning,
                "could not read FORALL_HOME config",
            )
            .details(details)
            .detail(err.to_string());
        }
    };

    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let candidates = [canonical.display().to_string(), root.display().to_string()];
    for path_str in candidates {
        let section_header = format!("[projects.\"{path_str}\"]");
        let Some(idx) = content.find(&section_header) else {
            continue;
        };
        let section = &content[idx..];
        let section_end = section[1..]
            .find("\n[projects.")
            .map(|offset| offset + 1)
            .unwrap_or(section.len());
        let section_body = &section[..section_end];
        details.push(format!("project entry: {path_str}"));
        if section_body.contains("trust_level = \"trusted\"") {
            return WorkflowDoctorCheck::new(
                "verified.trust",
                WorkflowDoctorStatus::Ok,
                "project is trusted in FORALL_HOME",
            )
            .details(details);
        }
        return WorkflowDoctorCheck::new(
            "verified.trust",
            WorkflowDoctorStatus::Warning,
            "project entry exists but is not trusted",
        )
        .details(details)
        .remediation(
            "Run `forall init` again or set trust_level = \"trusted\" in FORALL_HOME config.toml.",
        );
    }

    WorkflowDoctorCheck::new(
        "verified.trust",
        WorkflowDoctorStatus::Warning,
        "project not trusted in FORALL_HOME",
    )
    .details(details)
    .remediation("Run `forall init` in the project root to add a trusted entry.")
}

fn hooks_check(root: &Path) -> WorkflowDoctorCheck {
    let home_hooks = resolve_forall_home().join("hooks.json");
    let legacy_project_hooks = hooks_json_read_path(root);
    let hooks_json = if home_hooks.is_file() {
        home_hooks
    } else {
        legacy_project_hooks
    };
    let details = vec![format!(
        "hooks.json: {}",
        if hooks_json.is_file() {
            hooks_json.display().to_string()
        } else {
            "missing".to_string()
        }
    )];

    let status = if hooks_json.is_file() {
        WorkflowDoctorStatus::Ok
    } else {
        WorkflowDoctorStatus::Fail
    };

    let mut check = WorkflowDoctorCheck::new(
        "verified.hooks",
        status,
        match status {
            WorkflowDoctorStatus::Ok => "workflow hooks installed",
            WorkflowDoctorStatus::Warning => "workflow hooks need attention",
            WorkflowDoctorStatus::Fail => "required workflow hooks missing",
        },
    )
    .details(details);

    if status != WorkflowDoctorStatus::Ok {
        check =
            check.remediation("Run `forall init` or `forall hooks install` to regenerate hooks.");
    }
    check
}

fn cli_check(_root: &Path) -> WorkflowDoctorCheck {
    let config = load_verified_config(_root);
    let mut details = Vec::new();

    if let Ok(forall_cli) = env::var("FORALL_CLI") {
        details.push(format!("FORALL_CLI: {forall_cli}"));
    }
    if let Ok(forall_cli_js) = env::var("FORALL_CLI_JS") {
        details.push(format!("FORALL_CLI_JS: {forall_cli_js}"));
    }

    if let Some(cli_js) = config.cli_js.as_deref().filter(|s| !s.trim().is_empty()) {
        details.push(format!("legacy cli_js: {cli_js}"));
        let cli_path = PathBuf::from(cli_js);
        if !cli_path.is_file() {
            return WorkflowDoctorCheck::new(
                "verified.cli",
                WorkflowDoctorStatus::Fail,
                "legacy cli_js path not found",
            )
            .details(details)
            .remediation(
                "Remove cli_js from the legacy adapters file or set FORALL_CLI / install `forall` on PATH.",
            );
        }
        if !is_executable(&cli_path) {
            return WorkflowDoctorCheck::new(
                "verified.cli",
                WorkflowDoctorStatus::Warning,
                "legacy cli_js is not executable",
            )
            .details(details)
            .remediation("Fix permissions or use `forall` on PATH instead.");
        }
    } else {
        details.push("resolution: FORALL_CLI → FORALL_CLI_JS → `forall` on PATH".to_string());
    }

    if let Ok(current_exe) = env::current_exe() {
        details.push(format!("running binary: {}", current_exe.display()));
    }

    if which_forall_on_path() {
        details.push("`forall` found on PATH".to_string());
        return WorkflowDoctorCheck::new(
            "verified.cli",
            WorkflowDoctorStatus::Ok,
            "forall CLI resolves",
        )
        .details(details);
    }

    if env::var("FORALL_CLI").is_ok() || env::var("FORALL_CLI_JS").is_ok() {
        return WorkflowDoctorCheck::new(
            "verified.cli",
            WorkflowDoctorStatus::Ok,
            "forall CLI configured via env",
        )
        .details(details);
    }

    WorkflowDoctorCheck::new(
        "verified.cli",
        WorkflowDoctorStatus::Warning,
        "`forall` not found on PATH",
    )
    .details(details)
    .remediation("Install `forall` on PATH or set FORALL_CLI to the binary you use for hooks.")
}

fn which_forall_on_path() -> bool {
    std::process::Command::new("forall")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn adapters_check(root: &Path) -> WorkflowDoctorCheck {
    let config = load_verified_config(root);
    let mut details = vec![
        format!("backend: {}", config.backend),
        format!("strict: {}", config.strict),
        format!("adapters: {}", config.adapters.len()),
    ];
    for (name, adapter) in &config.adapters {
        details.push(format!(
            "{name}: proof={} extensions=[{}]",
            adapter.proof,
            adapter.extensions.join(", ")
        ));
    }

    WorkflowDoctorCheck::new(
        "verified.adapters",
        WorkflowDoctorStatus::Ok,
        "verification adapter registry loaded",
    )
    .details(details)
}

fn active_change_check(root: &Path) -> WorkflowDoctorCheck {
    let active = list_active_changes(root);
    let pointer = read_active_change_file(root);
    let env_override = env::var("FORALL_CHANGE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let resolved = resolve_active_change(root);
    let mut details = vec![format!("active changes: {}", active.len())];
    if !active.is_empty() {
        details.push(format!("changes: {}", active.join(", ")));
    }
    if let Some(name) = pointer.as_deref() {
        details.push(format!("workflow/active: {name}"));
    }
    if let Some(name) = env_override.as_deref() {
        details.push(format!("FORALL_CHANGE: {name}"));
    }

    if let Some(name) = resolved.as_deref() {
        details.push(format!("resolved active change: {name}"));
        if active.iter().any(|change| change == name) {
            return WorkflowDoctorCheck::new(
                "verified.active_change",
                WorkflowDoctorStatus::Ok,
                "active change resolved",
            )
            .details(details);
        }

        let (summary, remediation) = if env_override.as_deref() == Some(name) {
            (
                "FORALL_CHANGE points to a missing change",
                "Run `unset FORALL_CHANGE`, then `forall propose <name>` or point `.forall/workflow/active` at an active change.",
            )
        } else if pointer.as_deref() == Some(name) {
            (
                "workflow/active points to an archived or missing change",
                "Clear or update `.forall/workflow/active`, or run `forall propose <name>`.",
            )
        } else {
            (
                "resolved active change is not active",
                "Run `forall propose <name>` or update `.forall/workflow/active` to an active change.",
            )
        };
        return WorkflowDoctorCheck::new(
            "verified.active_change",
            WorkflowDoctorStatus::Warning,
            summary,
        )
        .details(details)
        .remediation(remediation);
    }

    if let Some(name) = pointer.as_deref()
        && !active.iter().any(|change| change == name)
    {
        return WorkflowDoctorCheck::new(
            "verified.active_change",
            WorkflowDoctorStatus::Warning,
            "workflow/active points to an archived or missing change",
        )
        .details(details)
        .remediation("Clear or update `.forall/workflow/active`, or run `forall propose <name>`.");
    }

    if active.is_empty() {
        return WorkflowDoctorCheck::new(
            "verified.active_change",
            WorkflowDoctorStatus::Warning,
            "no active change",
        )
        .details(details)
        .remediation("Run `forall propose <name>` before implementation edits.");
    }

    WorkflowDoctorCheck::new(
        "verified.active_change",
        WorkflowDoctorStatus::Warning,
        "multiple active changes — none selected",
    )
    .details(details)
    .remediation("Set FORALL_CHANGE or write `.forall/workflow/active`.")
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::workflow::run_init;
    use crate::test_env::with_forall_change;

    #[test]
    fn outside_project_reports_warning() {
        let dir = tempfile::tempdir().unwrap();
        let checks = run_verified_doctor(dir.path());
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].id, "verified.project");
        assert_eq!(checks[0].status, WorkflowDoctorStatus::Warning);
    }

    #[test]
    fn initialized_project_passes_core_checks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(run_init(root, false, false, false), 0);

        let checks = run_verified_doctor(root);
        let by_id: std::collections::BTreeMap<_, _> = checks
            .iter()
            .map(|check| (check.id.as_str(), check))
            .collect();

        assert_eq!(by_id["verified.project"].status, WorkflowDoctorStatus::Ok);
        assert_eq!(by_id["verified.hooks"].status, WorkflowDoctorStatus::Ok);
        assert_eq!(by_id["verified.adapters"].status, WorkflowDoctorStatus::Ok);
        assert!(
            by_id["verified.cli"].status == WorkflowDoctorStatus::Ok
                || by_id["verified.cli"].status == WorkflowDoctorStatus::Warning
        );
    }

    #[test]
    fn stale_active_change_pointer_reports_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(run_init(root, false, false, false), 0);
        with_forall_change(None, || {
            fs::write(crate::paths::workflow_active_path(root), "missing-change\n").unwrap();

            let checks = run_verified_doctor(root);
            let change = checks
                .iter()
                .find(|check| check.id == "verified.active_change")
                .expect("active change check");
            assert_eq!(change.status, WorkflowDoctorStatus::Warning);
            assert!(change.summary.contains("workflow/active"));
        });
    }

    #[test]
    fn forall_change_env_missing_change_reports_warning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(run_init(root, false, false, false), 0);
        with_forall_change(Some("ghost-change"), || {
            let checks = run_verified_doctor(root);
            let change = checks
                .iter()
                .find(|check| check.id == "verified.active_change")
                .expect("active change check");
            assert_eq!(change.status, WorkflowDoctorStatus::Warning);
            assert!(change.summary.contains("FORALL_CHANGE"));
        });
    }
}
