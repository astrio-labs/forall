//! Shared plugin package models, source providers, identifiers, and telemetry summaries.

pub use forall_utils_plugins::mention_syntax;
pub use forall_utils_plugins::plugin_namespace_for_skill_path;

mod load_outcome;
pub mod manifest;
mod plugin_id;
mod provider;

use forall_config::HookEventsToml;
use forall_utils_absolute_path::AbsolutePathBuf;
pub use load_outcome::EffectiveSkillRoots;
pub use load_outcome::LoadedPlugin;
pub use load_outcome::PluginLoadOutcome;
pub use load_outcome::prompt_safe_plugin_description;
pub use plugin_id::PluginId;
pub use plugin_id::PluginIdError;
pub use plugin_id::validate_plugin_segment;
pub use provider::PluginProvider;
pub use provider::PluginResourceLocator;
pub use provider::ResolvedPlugin;
pub use provider::ResolvedPluginError;
pub use provider::ResolvedPluginLocation;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginCapabilitySummary {
    pub config_name: String,
    pub display_name: String,
    pub description: Option<String>,
    pub has_skills: bool,
    pub mcp_server_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHookSource {
    pub plugin_id: PluginId,
    pub plugin_root: AbsolutePathBuf,
    pub plugin_data_root: AbsolutePathBuf,
    pub source_path: AbsolutePathBuf,
    pub source_relative_path: String,
    pub hooks: HookEventsToml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTelemetryMetadata {
    pub plugin_id: PluginId,
    /// Optional backend identifier for remote plugins, used when analytics
    /// should report the remote id instead of the local plugin cache id.
    pub remote_plugin_id: Option<String>,
    pub capability_summary: Option<PluginCapabilitySummary>,
}
