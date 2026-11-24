//! Shared implementation for `forall archive`, `forall delete`, and `forall unarchive`.
//!
//! The CLI commands are thin app-server clients: resolve a user-provided UUID or exact session
//! name, then call the corresponding app-server RPC.

use std::io::IsTerminal;
use std::io::Write;
use std::sync::Arc;

use crate::Cli;
use crate::app_server_session::AppServerSession;
use crate::legacy_core::config::ConfigBuilder;
use crate::legacy_core::config::ConfigOverrides;
use crate::legacy_core::config::load_config_toml_with_layer_stack;
use crate::legacy_core::config::resolve_oss_provider;
use crate::legacy_core::config::resolve_profile_v2_config_path;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use color_eyre::eyre::eyre;
use forall_app_server_protocol::Thread as AppServerThread;
use forall_app_server_protocol::ThreadListParams;
use forall_app_server_protocol::ThreadSortKey;
use forall_arg0::Arg0DispatchPaths;
use forall_config::CloudConfigBundleLoader;
use forall_config::ConfigLoadOptions;
use forall_config::LoaderOverrides;
use forall_exec_server::EnvironmentManager;
use forall_exec_server::ExecServerRuntimePaths;
use forall_protocol::ThreadId;
use forall_utils_cli::CliConfigOverrides;
use forall_utils_home_dir::find_forall_home;
use forall_utils_oss::get_default_model_for_oss_provider;

use super::RemoteAppServerEndpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteConfirmation {
    Prompt,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionArchiveAction {
    Archive,
    Delete(DeleteConfirmation),
    Unarchive,
}

pub struct SessionArchiveCommandOptions {
    pub cli: Cli,
    pub arg0_paths: Arg0DispatchPaths,
    pub explicit_remote_endpoint: Option<RemoteAppServerEndpoint>,
}

fn success_message(
    action: SessionArchiveAction,
    session_id: ThreadId,
    session_name: Option<&str>,
) -> String {
    let action = match action {
        SessionArchiveAction::Archive => "Archived",
        SessionArchiveAction::Delete(_) => "Deleted",
        SessionArchiveAction::Unarchive => "Unarchived",
    };
    match session_name {
        Some(name) => format!("{action} session {name} ({session_id})."),
        None => format!("{action} session {session_id}."),
    }
}

struct ResolvedSessionTarget {
    session_id: ThreadId,
    session_name: Option<String>,
}

pub async fn run_session_archive_command(
    action: SessionArchiveAction,
    target: String,
    options: SessionArchiveCommandOptions,
) -> Result<String> {
    let mut app_server = start_app_server_for_archive_command(options).await?;
    run_session_archive_action_with_app_server(&mut app_server, action, &target).await
}

async fn run_session_archive_action_with_app_server(
    app_server: &mut AppServerSession,
    action: SessionArchiveAction,
    target: &str,
) -> Result<String> {
    let resolved = resolve_session_target(app_server, action, target).await?;
    let session_name = match action {
        SessionArchiveAction::Archive => {
            app_server.thread_archive(resolved.session_id).await?;
            resolved.session_name
        }
        SessionArchiveAction::Delete(confirmation) => {
            if matches!(confirmation, DeleteConfirmation::Prompt)
                && !confirm_session_delete(&resolved)?
            {
                return Ok("Delete cancelled.".to_string());
            }
            app_server.thread_delete(resolved.session_id).await?;
            resolved.session_name
        }
        SessionArchiveAction::Unarchive => {
            let thread = app_server.thread_unarchive(resolved.session_id).await?;
            thread.name.or(resolved.session_name)
        }
    };
    Ok(success_message(
        action,
        resolved.session_id,
        session_name.as_deref(),
    ))
}

async fn resolve_session_target(
    app_server: &mut AppServerSession,
    action: SessionArchiveAction,
    target: &str,
) -> Result<ResolvedSessionTarget> {
    if let Ok(session_id) = ThreadId::from_string(target) {
        if matches!(
            action,
            SessionArchiveAction::Delete(DeleteConfirmation::Prompt)
        ) {
            let thread = app_server
                .thread_read(session_id, /*include_turns*/ false)
                .await
                .with_context(|| {
                    format!("No active or archived session found matching '{target}'.")
                })?;
            return Ok(ResolvedSessionTarget {
                session_id,
                session_name: thread.name,
            });
        }
        return Ok(ResolvedSessionTarget {
            session_id,
            session_name: None,
        });
    }

    let (search_scope, archived_values): (&str, &[bool]) = match action {
        SessionArchiveAction::Archive => ("active", &[false]),
        SessionArchiveAction::Delete(_) => ("active or archived", &[false, true]),
        SessionArchiveAction::Unarchive => ("archived", &[true]),
    };
    for &archived in archived_values {
        if let Some(thread) = lookup_session_by_exact_name(app_server, target, archived).await? {
            return session_target_from_app_server_thread(thread);
        }
    }
    Err(eyre!(
        "No {search_scope} session found matching '{target}'."
    ))
}

async fn lookup_session_by_exact_name(
    app_server: &mut AppServerSession,
    name: &str,
    archived: bool,
) -> Result<Option<AppServerThread>> {
    // Search is the fast path, but some stores attach renamed titles after applying the filter.
    for search_term in [Some(name), None] {
        let mut cursor = None;
        loop {
            let response = app_server
                .thread_list(ThreadListParams {
                    cursor: cursor.clone(),
                    limit: Some(100),
                    sort_key: Some(ThreadSortKey::UpdatedAt),
                    sort_direction: None,
                    model_providers: None,
                    source_kinds: Some(super::resume_source_kinds(
                        /*include_non_interactive*/ false,
                    )),
                    archived: Some(archived),
                    parent_thread_id: None,
                    cwd: None,
                    use_state_db_only: false,
                    search_term: search_term.map(str::to_string),
                })
                .await
                .wrap_err("failed to list sessions while resolving session name")?;

            if let Some(thread) = response
                .data
                .into_iter()
                .find(|thread| thread.name.as_deref() == Some(name))
            {
                return Ok(Some(thread));
            }
            let Some(next_cursor) = response.next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }
    }
    Ok(None)
}

fn session_target_from_app_server_thread(thread: AppServerThread) -> Result<ResolvedSessionTarget> {
    let session_id = ThreadId::from_string(&thread.id)
        .wrap_err_with(|| format!("app server returned invalid session id `{}`", thread.id))?;
    Ok(ResolvedSessionTarget {
        session_id,
        session_name: thread.name,
    })
}

fn confirm_session_delete(target: &ResolvedSessionTarget) -> Result<bool> {
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(eyre!(
            "cannot confirm session deletion without an interactive terminal; rerun with --force and a session UUID"
        ));
    }

    let mut stderr = std::io::stderr().lock();
    match target.session_name.as_deref() {
        Some(name) => writeln!(
            stderr,
            "Permanently delete session '{name}' ({})?",
            target.session_id
        ),
        None => writeln!(stderr, "Permanently delete session {}?", target.session_id),
    }?;
    writeln!(
        stderr,
        "This cannot be undone. Subagent threads will also be deleted."
    )?;
    write!(stderr, "Continue? [y/N]: ")?;
    stderr.flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let answer = input.trim();
    Ok(answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
}

async fn start_app_server_for_archive_command(
    options: SessionArchiveCommandOptions,
) -> Result<AppServerSession> {
    let SessionArchiveCommandOptions {
        cli,
        arg0_paths,
        explicit_remote_endpoint,
    } = options;
    let loader_overrides = LoaderOverrides::default();
    let strict_config = cli.strict_config;
    let raw_overrides = cli.config_overrides.raw_overrides.clone();
    let overrides_cli = CliConfigOverrides { raw_overrides };
    let cli_kv_overrides = overrides_cli
        .parse_overrides()
        .map_err(|err| eyre!("failed to parse -c overrides: {err}"))?;
    let forall_home = find_forall_home().wrap_err("failed to find Forall home")?;

    let mut launch_loader_overrides = loader_overrides.clone();
    if let Some(profile_v2) = cli.config_profile_v2.as_ref() {
        launch_loader_overrides.user_config_path = Some(resolve_profile_v2_config_path(
            forall_home.as_path(),
            profile_v2,
        ));
        launch_loader_overrides.user_config_profile = Some(profile_v2.clone());
    }

    let reuse_implicit_local_daemon = super::can_reuse_implicit_local_daemon(
        &cli_kv_overrides,
        &launch_loader_overrides,
        strict_config,
        cli.bypass_hook_trust,
    );
    let default_daemon = if explicit_remote_endpoint.is_none() && reuse_implicit_local_daemon {
        super::maybe_probe_default_daemon_socket(forall_home.as_path()).await
    } else {
        None
    };
    let app_server_target = super::app_server_target_for_launch(
        explicit_remote_endpoint,
        default_daemon,
        reuse_implicit_local_daemon,
    );
    let remote_cwd_override = cli
        .cwd
        .clone()
        .filter(|_| app_server_target.uses_remote_workspace());

    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        arg0_paths.forall_self_exe.clone(),
        arg0_paths.forall_linux_sandbox_exe.clone(),
    )
    .wrap_err("failed to resolve local runtime paths")?;
    let environment_manager = EnvironmentManager::from_env(Some(local_runtime_paths))
        .await
        .map(Arc::new)
        .wrap_err("failed to initialize environment manager")?;
    let config_cwd = super::config_cwd_for_app_server_target(
        cli.cwd.as_deref(),
        &app_server_target,
        &environment_manager,
    )
    .wrap_err("failed to resolve config cwd")?;

    let mut loader_overrides = loader_overrides;
    if let Some(profile_v2) = cli.config_profile_v2.as_ref() {
        loader_overrides.user_config_path = Some(resolve_profile_v2_config_path(
            forall_home.as_path(),
            profile_v2,
        ));
        loader_overrides.user_config_profile = Some(profile_v2.clone());
    }

    let bootstrap_config = load_config_toml_with_layer_stack(
        forall_home.as_path(),
        config_cwd.as_ref(),
        cli_kv_overrides.clone(),
        ConfigLoadOptions {
            loader_overrides: loader_overrides.clone(),
            strict_config,
            cloud_config_bundle: CloudConfigBundleLoader::default(),
        },
    )
    .await
    .wrap_err("failed to load config.toml")?;
    let config_toml = &bootstrap_config.config_toml;
    let cloud_config_bundle = CloudConfigBundleLoader::default();

    let model_provider = if cli.oss {
        resolve_oss_provider(cli.oss_provider.as_deref(), config_toml)
    } else {
        None
    };
    let model = cli.model.clone().or_else(|| {
        model_provider
            .as_deref()
            .and_then(get_default_model_for_oss_provider)
            .map(ToOwned::to_owned)
    });
    let cwd = cli.cwd.clone();
    let config = ConfigBuilder::default()
        .cli_overrides(cli_kv_overrides.clone())
        .harness_overrides(ConfigOverrides {
            model,
            cwd: if app_server_target.uses_remote_workspace() {
                None
            } else {
                cwd
            },
            model_provider,
            forall_self_exe: arg0_paths.forall_self_exe.clone(),
            forall_linux_sandbox_exe: arg0_paths.forall_linux_sandbox_exe.clone(),
            main_execve_wrapper_exe: arg0_paths.main_execve_wrapper_exe.clone(),
            show_raw_agent_reasoning: cli.oss.then_some(true),
            bypass_hook_trust: cli.bypass_hook_trust.then_some(true),
            ..Default::default()
        })
        .loader_overrides(loader_overrides.clone())
        .strict_config(strict_config)
        .cloud_config_bundle(cloud_config_bundle.clone())
        .build()
        .await
        .wrap_err("failed to load configuration")?;
    let state_db = super::init_state_db_for_app_server_target(&config, &app_server_target)
        .await
        .wrap_err("failed to initialize state database")?;
    let app_server = super::start_app_server(
        &app_server_target,
        arg0_paths,
        config,
        cli_kv_overrides,
        loader_overrides,
        strict_config,
        cloud_config_bundle,
        forall_feedback::ForallFeedback::new(),
        /*log_db*/ None,
        state_db,
        environment_manager,
    )
    .await?;
    Ok(
        AppServerSession::new(app_server, app_server_target.thread_params_mode())
            .with_remote_cwd_override(remote_cwd_override),
    )
}
