//! CLI login commands and their direct-user observability surfaces.
//!
//! The TUI path already installs a broader tracing stack with feedback, OpenTelemetry, and other
//! interactive-session layers. Direct `forall login` intentionally does less: it preserves the
//! existing stderr UX and adds only a small file-backed tracing layer for login-specific
//! targets. Keeping that setup local avoids pulling the TUI's session-oriented logging machinery
//! into a one-shot CLI command while still producing a durable `forall-login.log` artifact that
//! support can request from users.

use forall_app_server_protocol::AuthMode;
use forall_config::types::AuthCredentialsStoreMode;
use forall_core::config::Config;
use forall_login::AuthBaseUrls;
use forall_login::AuthKeyringBackendKind;
use forall_login::AuthRouteConfig;
use forall_login::ForallAuth;
use forall_login::OauthLoginFlow;
use forall_login::OauthLoginOptions;
use forall_login::VerificationApiKeyStore;
use forall_login::logout_with_revoke;
use forall_login::revoke_stored_oauth_tokens_best_effort;
use forall_login::set_openai_api_key;
use forall_utils_cli::CliConfigOverrides;
use std::fs::OpenOptions;
use std::io::IsTerminal;
use std::io::Read;
use std::path::Path;
use tracing_appender::non_blocking;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const API_KEY_CONFIGURED_MESSAGE: &str = "Successfully saved API key";

/// Installs a small file-backed tracing layer for direct `forall login` flows.
///
/// This deliberately duplicates a narrow slice of the TUI logging setup instead of reusing it
/// wholesale. The TUI stack includes session-oriented layers that are valuable for interactive
/// runs but unnecessary for a one-shot login command. Keeping the direct CLI path local lets this
/// command produce a durable `forall-login.log` artifact without coupling it to the TUI's broader
/// telemetry and feedback initialization.
fn init_login_file_logging(config: &Config) -> Option<WorkerGuard> {
    let log_dir = match forall_core::config::log_dir(config) {
        Ok(log_dir) => log_dir,
        Err(err) => {
            eprintln!("Warning: failed to resolve login log directory: {err}");
            return None;
        }
    };

    if let Err(err) = std::fs::create_dir_all(&log_dir) {
        eprintln!(
            "Warning: failed to create login log directory {}: {err}",
            log_dir.display()
        );
        return None;
    }

    let mut log_file_opts = OpenOptions::new();
    log_file_opts.create(true).append(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        log_file_opts.mode(0o600);
    }

    let log_path = log_dir.join("forall-login.log");
    let log_file = match log_file_opts.open(&log_path) {
        Ok(log_file) => log_file,
        Err(err) => {
            eprintln!(
                "Warning: failed to open login log file {}: {err}",
                log_path.display()
            );
            return None;
        }
    };

    let (non_blocking, guard) = non_blocking(log_file);
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("forall_cli=info,forall_core=info,forall_login=info"));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_target(true)
        .with_ansi(false)
        .with_filter(env_filter);

    // Direct `forall login` otherwise relies on ephemeral stderr output.
    // Persist the same login targets to a file so support can inspect auth failures
    // without reproducing them through TUI or app-server.
    if let Err(err) = tracing_subscriber::registry().with(file_layer).try_init() {
        eprintln!(
            "Warning: failed to initialize login log file {}: {err}",
            log_path.display()
        );
        return None;
    }

    Some(guard)
}

async fn clear_existing_auth_before_login(
    forall_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    auth_keyring_backend_kind: AuthKeyringBackendKind,
    auth_route_config: Option<&AuthRouteConfig>,
) {
    if let Err(err) = logout_with_revoke(
        forall_home,
        auth_credentials_store_mode,
        auth_keyring_backend_kind,
        auth_route_config,
    )
    .await
    {
        tracing::warn!("failed to clear existing auth before login: {err}");
    }
}

/// `forall git-credential-github <operation>` — the credential helper the
/// engine's git credential config points at. Reads git's `key=value` request
/// from stdin and answers `get` for `https`/`github.com` from the stored
/// connection. Always exits 0: a helper with nothing to say must let git
/// move on, not fail the whole git command.
pub async fn run_git_credential_github(
    cli_config_overrides: CliConfigOverrides,
    operation: String,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let connection = forall_login::load_github_connection(
        &config.forall_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    )
    .unwrap_or_default();

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(0);
    }
    let mut stdout = std::io::stdout();
    let _ = forall_login::github_connect::respond_to_git_credential_request(
        &operation,
        &input,
        connection.as_ref(),
        &mut stdout,
    );
    std::process::exit(0);
}

const OAUTH_LOGIN_STATUS: &str = "Logged in with your Forall account";

pub async fn run_login_default(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let _login_log_guard = init_login_file_logging(&config);
    tracing::info!("starting Forall OAuth login flow");

    let base_urls = AuthBaseUrls::resolve(config.forall_auth_base_url.as_deref());
    let options = OauthLoginOptions::new(
        base_urls,
        config.forall_home.to_path_buf(),
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    );

    let started = match OauthLoginFlow::start(options) {
        Ok(started) => started,
        Err(err) => {
            eprintln!("Error starting login: {err}");
            std::process::exit(1);
        }
    };

    eprintln!("Finish signing in with your Forall account in the browser.");
    eprintln!("If the browser did not open, visit:");
    eprintln!("  {}", started.auth_url);

    match started.completion.await {
        Ok(Ok(_)) => {
            eprintln!("{OAUTH_LOGIN_STATUS}");
            std::process::exit(0);
        }
        Ok(Err(err)) => {
            eprintln!("Login failed: {err}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Login failed: the login flow terminated unexpectedly");
            std::process::exit(1);
        }
    }
}

pub async fn run_login_with_api_key(
    cli_config_overrides: CliConfigOverrides,
    api_key: String,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let _login_log_guard = init_login_file_logging(&config);
    tracing::info!("starting API key setup flow");

    clear_existing_auth_before_login(
        &config.forall_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
        config.auth_route_config().as_ref(),
    )
    .await;

    match set_openai_api_key(
        &config.forall_home,
        &api_key,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    ) {
        Ok(()) => {
            eprintln!("{API_KEY_CONFIGURED_MESSAGE}");
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("Error saving API key: {err}");
            std::process::exit(1);
        }
    }
}

pub fn read_api_key_from_stdin() -> String {
    read_stdin_secret(
        "--with-api-key expects the API key on stdin. Try piping it, e.g. `printenv OPENAI_API_KEY | forall login --with-api-key`.",
        "Reading API key from stdin...",
        "No API key provided via stdin.",
    )
}

pub fn read_verification_api_key_from_stdin() -> String {
    read_stdin_secret(
        "verification login expects the API key on stdin. Create a key at https://forall.astrio.app/dashboard then try `printenv FORALL_API_KEY | forall verification login`.",
        "Reading Forall API key from stdin...",
        "No Forall API key provided via stdin.",
    )
}

pub async fn run_verification_login(
    cli_config_overrides: CliConfigOverrides,
    api_key: String,
) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let store = VerificationApiKeyStore::new(&config.forall_home);
    match store.set(&api_key) {
        Ok(()) => {
            eprintln!("Successfully saved Forall API key");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("Error saving Forall API key: {error}");
            std::process::exit(1);
        }
    }
}

pub async fn run_verification_status(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let store = VerificationApiKeyStore::new(&config.forall_home);
    match store.get() {
        Ok(Some(_)) => {
            eprintln!("Forall API key is configured");
            std::process::exit(0);
        }
        Ok(None) => {
            eprintln!(
                "Forall API key is not configured. Create one at https://forall.astrio.app/dashboard then pipe it to `forall verification login`."
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("Error checking Forall API key: {error}");
            std::process::exit(1);
        }
    }
}

pub async fn run_verification_logout(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let store = VerificationApiKeyStore::new(&config.forall_home);
    match store.delete() {
        Ok(true) => {
            eprintln!("Removed Forall API key");
            std::process::exit(0);
        }
        Ok(false) => {
            eprintln!("Forall API key was not configured");
            std::process::exit(0);
        }
        Err(error) => {
            eprintln!("Error removing Forall API key: {error}");
            std::process::exit(1);
        }
    }
}

fn read_stdin_secret(terminal_message: &str, reading_message: &str, empty_message: &str) -> String {
    let mut stdin = std::io::stdin();

    if stdin.is_terminal() {
        eprintln!("{terminal_message}");
        std::process::exit(1);
    }

    eprintln!("{reading_message}");

    let mut buffer = String::new();
    if let Err(err) = stdin.read_to_string(&mut buffer) {
        eprintln!("Failed to read stdin: {err}");
        std::process::exit(1);
    }

    let secret = buffer.trim().to_string();
    if secret.is_empty() {
        eprintln!("{empty_message}");
        std::process::exit(1);
    }

    secret
}

pub async fn run_login_status(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let auth_route_config = config.auth_route_config();

    match ForallAuth::from_auth_storage(
        &config.forall_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
        auth_route_config.as_ref(),
    )
    .await
    {
        Ok(Some(auth)) => match auth.auth_mode() {
            AuthMode::ApiKey => {
                if auth.api_key().is_some() {
                    eprintln!("Logged in using an API key");
                } else {
                    eprintln!("Logged in using provider-specific API keys");
                }
                std::process::exit(0);
            }
            AuthMode::BedrockApiKey => {
                eprintln!("Logged in using Amazon Bedrock API key");
                std::process::exit(0);
            }
            AuthMode::Oauth => {
                eprintln!("{OAUTH_LOGIN_STATUS}");
                std::process::exit(0);
            }
        },
        Ok(None) => {
            eprintln!("Not logged in");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error checking login status: {e}");
            std::process::exit(1);
        }
    }
}

pub async fn run_logout(cli_config_overrides: CliConfigOverrides) -> ! {
    let config = load_config_or_exit(cli_config_overrides).await;
    let auth_route_config = config.auth_route_config();

    // Best-effort server-side revocation; local logout proceeds regardless.
    revoke_stored_oauth_tokens_best_effort(
        &config.forall_home,
        &AuthBaseUrls::resolve(config.forall_auth_base_url.as_deref()),
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
    )
    .await;

    match logout_with_revoke(
        &config.forall_home,
        config.cli_auth_credentials_store_mode,
        config.auth_keyring_backend_kind(),
        auth_route_config.as_ref(),
    )
    .await
    {
        Ok(true) => {
            eprintln!("Successfully logged out");
            std::process::exit(0);
        }
        Ok(false) => {
            eprintln!("Not logged in");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error logging out: {e}");
            std::process::exit(1);
        }
    }
}

async fn load_config_or_exit(cli_config_overrides: CliConfigOverrides) -> Config {
    let cli_overrides = match cli_config_overrides.parse_overrides() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error parsing -c overrides: {e}");
            std::process::exit(1);
        }
    };

    match Config::load_with_cli_overrides(cli_overrides).await {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Error loading configuration: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use forall_config::types::AuthCredentialsStoreMode;
    use forall_login::AuthKeyringBackendKind;
    use forall_login::load_auth_dot_json;
    use forall_login::login_with_api_key;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    use super::clear_existing_auth_before_login;

    #[tokio::test]
    async fn clears_existing_auth_before_login() {
        let forall_home = tempdir().expect("create temporary Forall home");
        login_with_api_key(
            forall_home.path(),
            "sk-existing",
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("save existing auth");

        clear_existing_auth_before_login(
            forall_home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
            /*auth_route_config*/ None,
        )
        .await;

        let auth = load_auth_dot_json(
            forall_home.path(),
            AuthCredentialsStoreMode::File,
            AuthKeyringBackendKind::default(),
        )
        .expect("load auth after cleanup");
        assert_eq!(auth, None);
    }
}
