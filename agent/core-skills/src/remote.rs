use anyhow::Result;
use std::path::PathBuf;

use forall_login::ForallAuth;

const REMOTE_SKILLS_UNAVAILABLE: &str =
    "remote skills require a hosted backend, which is not configured";

// Low-level client for the remote skill API. Hosted-backend skill scopes are not
// available in BYOK builds.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSkillScope {
    WorkspaceShared,
    AllShared,
    Personal,
    Example,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSkillProductSurface {
    Chatgpt,
    Forall,
    Api,
    Atlas,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkillDownloadResult {
    pub id: String,
    pub path: PathBuf,
}

pub async fn list_remote_skills(
    auth: Option<&ForallAuth>,
    _scope: RemoteSkillScope,
    _product_surface: RemoteSkillProductSurface,
    _enabled: Option<bool>,
) -> Result<Vec<RemoteSkillSummary>> {
    let _ = auth;
    anyhow::bail!(REMOTE_SKILLS_UNAVAILABLE)
}

pub async fn export_remote_skill(
    _forall_home: PathBuf,
    auth: Option<&ForallAuth>,
    _skill_id: &str,
) -> Result<RemoteSkillDownloadResult> {
    let _ = auth;
    anyhow::bail!(REMOTE_SKILLS_UNAVAILABLE)
}
