use clap::Parser;
use forall_app_server::AppServerRuntimeOptions;
use forall_app_server::AppServerTransport;
use forall_app_server::AppServerWebsocketAuthArgs;
use forall_app_server::PluginStartupTasks;
use forall_app_server::run_main_with_transport_options;
use forall_arg0::Arg0DispatchPaths;
use forall_arg0::arg0_dispatch_or_else;
use forall_config::LoaderOverrides;
use forall_protocol::protocol::SessionSource;
use forall_utils_cli::CliConfigOverrides;
use std::path::PathBuf;

// Debug-only test hook: lets integration tests point the server at a temporary
// managed config file without writing to /etc.
const MANAGED_CONFIG_PATH_ENV_VAR: &str = "CODEX_APP_SERVER_MANAGED_CONFIG_PATH";
const DISABLE_MANAGED_CONFIG_ENV_VAR: &str = "CODEX_APP_SERVER_DISABLE_MANAGED_CONFIG";

#[derive(Debug, Parser)]
#[command(version)]
struct AppServerArgs {
    #[command(flatten)]
    config_overrides: CliConfigOverrides,

    /// Transport endpoint URL. Supported values: `stdio://` (default),
    /// `unix://`, `unix://PATH`, `ws://IP:PORT`, `off`.
    #[arg(
        long = "listen",
        value_name = "URL",
        default_value = AppServerTransport::DEFAULT_LISTEN_URL
    )]
    listen: AppServerTransport,

    /// Session source used to derive product restrictions and metadata.
    #[arg(
        long = "session-source",
        value_name = "SOURCE",
        default_value = "vscode",
        value_parser = SessionSource::from_startup_arg
    )]
    session_source: SessionSource,

    #[command(flatten)]
    auth: AppServerWebsocketAuthArgs,

    /// Fail if config.toml contains unknown configuration fields.
    #[arg(long = "strict-config", default_value_t = false)]
    strict_config: bool,

    /// Hidden debug-only test hook used by integration tests that spawn the
    /// production app-server binary.
    #[cfg(debug_assertions)]
    #[arg(long = "disable-plugin-startup-tasks-for-tests", hide = true)]
    disable_plugin_startup_tasks_for_tests: bool,
}

fn main() -> anyhow::Result<()> {
    arg0_dispatch_or_else(move |arg0_paths: Arg0DispatchPaths| async move {
        let AppServerArgs {
            config_overrides,
            listen,
            session_source,
            auth,
            strict_config,
            #[cfg(debug_assertions)]
            disable_plugin_startup_tasks_for_tests,
        } = AppServerArgs::parse();
        let loader_overrides = if disable_managed_config_from_debug_env() {
            LoaderOverrides::without_managed_config_for_tests()
        } else {
            managed_config_path_from_debug_env()
                .map(LoaderOverrides::with_managed_config_path_for_tests)
                .unwrap_or_default()
        };
        let transport = listen;
        let auth = auth.try_into_settings()?;
        let mut runtime_options = AppServerRuntimeOptions::default();
        #[cfg(debug_assertions)]
        if disable_plugin_startup_tasks_for_tests {
            runtime_options.plugin_startup_tasks = PluginStartupTasks::Skip;
        }

        run_main_with_transport_options(
            arg0_paths,
            config_overrides,
            loader_overrides,
            strict_config,
            transport,
            session_source,
            auth,
            runtime_options,
        )
        .await
        .map_err(anyhow::Error::from)
    })
}

fn managed_config_path_from_debug_env() -> Option<PathBuf> {
    std::env::var_os(MANAGED_CONFIG_PATH_ENV_VAR).map(PathBuf::from)
}

fn disable_managed_config_from_debug_env() -> bool {
    std::env::var_os(DISABLE_MANAGED_CONFIG_ENV_VAR)
        .map(|value| value == "1")
        .unwrap_or(false)
}
