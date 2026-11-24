use forall_core::config::Config;
use forall_extension_api::ExtensionRegistryBuilder;

mod executor_plugin;

/// Installs discovery for MCP servers declared by thread-selected executor plugins.
pub fn install_executor_plugins(
    builder: &mut ExtensionRegistryBuilder<Config>,
    environment_manager: std::sync::Arc<forall_exec_server::EnvironmentManager>,
) {
    builder.mcp_server_contributor(std::sync::Arc::new(
        executor_plugin::SelectedExecutorPluginMcpContributor::new(environment_manager),
    ));
}

/// Seeds the per-thread snapshot used by selected executor plugin MCP discovery.
pub fn initialize_executor_plugin_thread_data(
    thread_init: &mut forall_extension_api::ExtensionDataInit,
) {
    executor_plugin::seed_thread_state(thread_init);
}
