use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use forall_config::McpServerConfig;
use forall_core_plugins::PluginsManager;
use forall_extension_api::ExtensionDataInit;
use forall_extension_api::ExtensionRegistry;
use forall_extension_api::McpServerContribution;
use forall_extension_api::McpServerContributionContext;
use forall_features::Feature;
use forall_login::ForallAuth;
use forall_mcp::EffectiveMcpServer;
use forall_mcp::McpConfig;
use forall_mcp::McpPluginAttribution;
use forall_mcp::McpServerRegistration;
use forall_mcp::PLAYWRIGHT_MCP_SERVER_NAME;
use forall_mcp::configured_mcp_servers;
use forall_mcp::effective_mcp_servers;
use forall_mcp::playwright_mcp_server_config;

const CODEX_APPS_MCP_SERVER_NAME: &str = "forall_apps";
const LEGACY_CODEX_APPS_REGISTRATION_ID: &str = "legacy_forall_apps";
const BUILTIN_PLAYWRIGHT_REGISTRATION_ID: &str = "builtin_playwright";

enum OrderedMcpOverlay {
    Set {
        contributor_id: &'static str,
        contribution_order: usize,
        name: String,
        config: Box<McpServerConfig>,
    },
    Remove {
        contributor_id: &'static str,
        contribution_order: usize,
        name: String,
    },
}

#[derive(Clone)]
pub struct McpManager {
    plugins_manager: Arc<PluginsManager>,
    extensions: Arc<ExtensionRegistry<Config>>,
}

impl McpManager {
    pub fn new(plugins_manager: Arc<PluginsManager>) -> Self {
        Self {
            plugins_manager,
            extensions: forall_extension_api::empty_extension_registry(),
        }
    }

    /// Creates a manager that resolves host-installed MCP contributions.
    pub fn new_with_extensions(
        plugins_manager: Arc<PluginsManager>,
        extensions: Arc<ExtensionRegistry<Config>>,
    ) -> Self {
        Self {
            plugins_manager,
            extensions,
        }
    }

    /// Returns the MCP config after applying compatibility built-ins and
    /// runtime-only extension overlays.
    pub async fn runtime_config(&self, config: &Config) -> McpConfig {
        self.runtime_config_with_context(config, /*thread_init*/ None)
            .await
    }

    pub(crate) async fn runtime_config_for_thread(
        &self,
        config: &Config,
        thread_init: &ExtensionDataInit,
    ) -> McpConfig {
        self.runtime_config_with_context(config, Some(thread_init))
            .await
    }

    async fn runtime_config_with_context(
        &self,
        config: &Config,
        thread_init: Option<&ExtensionDataInit>,
    ) -> McpConfig {
        let context = match thread_init {
            Some(thread_init) => McpServerContributionContext::for_thread(config, thread_init),
            None => McpServerContributionContext::global(config),
        };
        let mut selected_plugin_registrations = Vec::new();
        let mut overlays = Vec::new();
        // A contributor can emit multiple ordered actions, so order each action globally rather
        // than enumerating contributors.
        let mut contribution_order = 0;
        for contributor in self.extensions.mcp_server_contributors() {
            for contribution in contributor.contribute(context).await {
                match contribution {
                    McpServerContribution::Set { name, config } => {
                        overlays.push(OrderedMcpOverlay::Set {
                            contributor_id: contributor.id(),
                            contribution_order,
                            name,
                            config,
                        });
                    }
                    McpServerContribution::SelectedPlugin {
                        name,
                        plugin_id,
                        plugin_display_name,
                        selection_order,
                        config,
                    } => selected_plugin_registrations.push(
                        McpServerRegistration::from_selected_plugin(
                            name,
                            McpPluginAttribution::new(plugin_id, plugin_display_name),
                            selection_order,
                            *config,
                        ),
                    ),
                    McpServerContribution::Remove { name } => {
                        overlays.push(OrderedMcpOverlay::Remove {
                            contributor_id: contributor.id(),
                            contribution_order,
                            name,
                        });
                    }
                }
                contribution_order += 1;
            }
        }

        let mut mcp_config = config
            .to_mcp_config_with_plugin_registrations(
                self.plugins_manager.as_ref(),
                selected_plugin_registrations,
            )
            .await;
        let mut catalog = mcp_config.mcp_server_catalog.to_builder();
        catalog.remove_compatibility(
            CODEX_APPS_MCP_SERVER_NAME.to_string(),
            LEGACY_CODEX_APPS_REGISTRATION_ID,
        );
        if config.features.enabled(Feature::ComputerUse) {
            catalog.register(McpServerRegistration::from_compatibility(
                PLAYWRIGHT_MCP_SERVER_NAME.to_string(),
                BUILTIN_PLAYWRIGHT_REGISTRATION_ID,
                playwright_mcp_server_config(),
            ));
        } else {
            catalog.remove_compatibility(
                PLAYWRIGHT_MCP_SERVER_NAME.to_string(),
                BUILTIN_PLAYWRIGHT_REGISTRATION_ID,
            );
        }

        for overlay in overlays {
            match overlay {
                OrderedMcpOverlay::Set {
                    contributor_id,
                    contribution_order,
                    name,
                    config,
                } => catalog.register(McpServerRegistration::from_extension(
                    name,
                    contributor_id,
                    contribution_order,
                    *config,
                )),
                OrderedMcpOverlay::Remove {
                    contributor_id,
                    contribution_order,
                    name,
                } => catalog.remove_extension(name, contributor_id, contribution_order),
            }
        }
        let catalog = catalog.build();
        for conflict in catalog.conflicts() {
            tracing::warn!(
                server = conflict.name,
                outcome = ?conflict.outcome,
                contenders = ?conflict.contenders,
                "conflicting MCP server actions; using resolved catalog outcome"
            );
        }
        mcp_config.mcp_server_catalog = catalog;
        mcp_config
    }

    /// Returns config- and plugin-backed servers without runtime contributions.
    pub async fn configured_servers(&self, config: &Config) -> HashMap<String, McpServerConfig> {
        let mcp_config = config.to_mcp_config(self.plugins_manager.as_ref()).await;
        configured_mcp_servers(&mcp_config)
    }

    /// Returns configured and host-contributed servers before auth gating.
    pub async fn runtime_servers(&self, config: &Config) -> HashMap<String, McpServerConfig> {
        let mcp_config = self.runtime_config(config).await;
        configured_mcp_servers(&mcp_config)
    }

    /// Returns runtime servers after auth gating and compatibility built-ins.
    pub async fn effective_servers(
        &self,
        config: &Config,
        auth: Option<&ForallAuth>,
    ) -> HashMap<String, EffectiveMcpServer> {
        let mcp_config = self.runtime_config(config).await;
        effective_mcp_servers(&mcp_config, auth)
    }
}
