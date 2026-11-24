use super::ForallClient;
use super::shell_quote;
use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use forall_app_server_protocol::ClientRequest;
use forall_app_server_protocol::PluginReadParams;
use forall_app_server_protocol::PluginReadResponse;
use forall_app_server_protocol::PluginUninstallParams;
use forall_app_server_protocol::PluginUninstallResponse;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const REMOTE_MARKETPLACE_HINT: &str = "openai-curated-remote";
const STATE_TIMEOUT: Duration = Duration::from_secs(15);

const POLL_INTERVAL: Duration = Duration::from_millis(100);

pub(super) fn run_cleanup(
    forall_bin: &Path,
    config_overrides: &[String],
    remote_plugin_id: &str,
    confirmation: AccountMutationConfirmation,
) -> Result<()> {
    require_confirmation(confirmation)?;
    let mut overrides = config_overrides.to_vec();
    overrides.extend([
        "features.plugins=true".to_string(),
        "features.remote_plugin=true".to_string(),
    ]);
    let mut client = ForallClient::spawn_stdio(forall_bin, &overrides)?;
    client.initialize()?;

    match restore_uninstalled_state(&mut client, remote_plugin_id) {
        RestorationStatus::Clean => {
            println!("PASS: `{remote_plugin_id}` is uninstalled");
            Ok(())
        }
        RestorationStatus::LocalCleanupFailure(err) => {
            eprintln!(
                "FAIL-LOCAL-CACHE: backend state is uninstalled, but local cleanup reported an error: {err:#}"
            );
            Err(err)
        }
        RestorationStatus::Dirty(err) => {
            print_dirty_recovery(forall_bin, config_overrides, remote_plugin_id, &err);
            Err(err)
        }
        RestorationStatus::Unknown(err) => {
            eprintln!(
                "FAIL-UNKNOWN: could not verify whether `{remote_plugin_id}` is installed: {err:#}"
            );
            Err(err)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum AccountMutationConfirmation {
    Confirmed,
    Missing,
}

impl AccountMutationConfirmation {
    pub(super) fn from_flag(confirm_account_mutation: bool) -> Self {
        if confirm_account_mutation {
            Self::Confirmed
        } else {
            Self::Missing
        }
    }
}

fn require_confirmation(confirmation: AccountMutationConfirmation) -> Result<()> {
    if matches!(confirmation, AccountMutationConfirmation::Missing) {
        bail!(
            "this command installs and uninstalls a plugin on the active account; rerun with --confirm-account-mutation"
        );
    }
    Ok(())
}

fn is_plugin_installed(client: &mut ForallClient, remote_plugin_id: &str) -> Result<bool> {
    let request_id = client.request_id();
    let response: PluginReadResponse = client.send_request(
        ClientRequest::PluginRead {
            request_id: request_id.clone(),
            params: PluginReadParams {
                marketplace_path: None,
                remote_marketplace_name: Some(REMOTE_MARKETPLACE_HINT.to_string()),
                plugin_name: remote_plugin_id.to_string(),
            },
        },
        request_id,
        "plugin/read",
    )?;
    let summary = response.plugin.summary;
    let actual_remote_plugin_id = summary
        .remote_plugin_id
        .with_context(|| format!("plugin/read returned no remote id for `{remote_plugin_id}`"))?;
    if actual_remote_plugin_id != remote_plugin_id {
        bail!(
            "plugin/read returned remote id `{actual_remote_plugin_id}` for requested id `{remote_plugin_id}`"
        );
    }
    Ok(summary.installed)
}

fn uninstall_remote_plugin(client: &mut ForallClient, remote_plugin_id: &str) -> Result<()> {
    let request_id = client.request_id();
    let _: PluginUninstallResponse = client.send_request(
        ClientRequest::PluginUninstall {
            request_id: request_id.clone(),
            params: PluginUninstallParams {
                plugin_id: remote_plugin_id.to_string(),
            },
        },
        request_id,
        "plugin/uninstall",
    )?;
    Ok(())
}

fn wait_until_uninstalled(client: &mut ForallClient, remote_plugin_id: &str) -> Result<()> {
    let deadline = Instant::now() + STATE_TIMEOUT;
    loop {
        match is_plugin_installed(client, remote_plugin_id) {
            Ok(false) => return Ok(()),
            Ok(_) => {}
            Err(err) if Instant::now() >= deadline => return Err(err),
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for remote plugin `{remote_plugin_id}` to be uninstalled");
        }
        thread::sleep(POLL_INTERVAL);
    }
}

enum RestorationStatus {
    Clean,
    LocalCleanupFailure(anyhow::Error),
    Dirty(anyhow::Error),
    Unknown(anyhow::Error),
}

fn restore_uninstalled_state(
    client: &mut ForallClient,
    remote_plugin_id: &str,
) -> RestorationStatus {
    let current = match is_plugin_installed(client, remote_plugin_id) {
        Ok(current) => current,
        Err(err) => return RestorationStatus::Unknown(err),
    };
    if !current {
        return RestorationStatus::Clean;
    }

    let uninstall_result = uninstall_remote_plugin(client, remote_plugin_id);
    match wait_until_uninstalled(client, remote_plugin_id) {
        Ok(_) => match uninstall_result {
            Ok(()) => RestorationStatus::Clean,
            Err(err) => RestorationStatus::LocalCleanupFailure(err),
        },
        Err(state_err) => {
            let error = match uninstall_result {
                Ok(()) => state_err,
                Err(uninstall_err) => anyhow!(
                    "cleanup uninstall failed: {uninstall_err:#}; state verification failed: {state_err:#}"
                ),
            };
            RestorationStatus::Dirty(error)
        }
    }
}

fn print_dirty_recovery(
    forall_bin: &Path,
    config_overrides: &[String],
    remote_plugin_id: &str,
    err: &anyhow::Error,
) {
    eprintln!(
        "FAIL-DIRTY: remote plugin `{remote_plugin_id}` still appears installed after cleanup: {err:#}"
    );
    print_recovery_command(forall_bin, config_overrides, remote_plugin_id);
}

fn print_recovery_command(forall_bin: &Path, config_overrides: &[String], remote_plugin_id: &str) {
    let test_client = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "forall-app-server-test-client".to_string());
    let mut command = format!(
        "{} --forall-bin {}",
        shell_quote(&test_client),
        shell_quote(&forall_bin.display().to_string())
    );
    for override_kv in config_overrides {
        command.push_str(&format!(" --config {}", shell_quote(override_kv)));
    }
    command.push_str(&format!(
        " plugin-remote-uninstall --remote-plugin-id {} --confirm-account-mutation",
        shell_quote(remote_plugin_id)
    ));
    eprintln!("Recovery command:");
    eprintln!("  {command}");
}
