//! Workspace-level feature flags from a hosted backend.
//!
//! Forall BYOK deployments do not call ChatGPT workspace APIs; plugins stay enabled locally.

use crate::config::Config;
use forall_login::ForallAuth;

#[derive(Debug, Default)]
pub struct WorkspaceSettingsCache;

pub async fn forall_plugins_enabled_for_workspace(
    _config: &Config,
    _auth: Option<&ForallAuth>,
    _cache: Option<&WorkspaceSettingsCache>,
) -> anyhow::Result<bool> {
    Ok(true)
}
