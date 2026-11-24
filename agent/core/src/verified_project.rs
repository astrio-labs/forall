//! Eager verified-project bootstrap when the agent starts in a workspace without `.forall/`.

use std::path::Path;

use forall_workflow::ensure_verified_project;
use forall_workflow::paths::find_project_root;
use tracing::info;

/// Initialize `.forall/` when the workspace has no Forall project yet.
///
/// Idempotent: no-op when a project root already exists. Matches M0.4 policy
/// ("eager when workspace obvious") for `forall` sessions opened in a cwd.
pub(crate) fn maybe_ensure_verified_project_for_workspace(cwd: &Path) -> bool {
    let resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if find_project_root(&resolved).is_some() {
        return false;
    }

    let result = ensure_verified_project(&resolved);
    if result.created {
        info!(
            project_root = %result.project_root,
            "initialized Forall project for workspace"
        );
        true
    } else if result.ok {
        false
    } else {
        tracing::warn!(
            cwd = %resolved.display(),
            "forall ensure failed during session startup; app-code gates may block until `forall init` succeeds"
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forall_workflow::commands::workflow::run_init;

    #[test]
    fn ensure_creates_forall_tree_in_empty_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(maybe_ensure_verified_project_for_workspace(dir.path()));
        assert!(dir.path().join(".forall/verify/mapping.yaml").is_file());
        assert!(!maybe_ensure_verified_project_for_workspace(dir.path()));
    }

    #[test]
    fn ensure_is_noop_when_project_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(run_init(dir.path(), false, false, false), 0);
        assert!(!maybe_ensure_verified_project_for_workspace(dir.path()));
    }
}
