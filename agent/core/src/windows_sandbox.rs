use crate::config::Config;
use crate::config::edit::ConfigEditsBuilder;
use forall_config::config_toml::ConfigToml;
use forall_config::types::WindowsSandboxModeToml;
use forall_features::Feature;
use forall_features::Features;
use forall_features::FeaturesToml;
use forall_login::default_client::originator;
use forall_otel::sanitize_metric_tag_value;
use forall_protocol::config_types::WindowsSandboxLevel;
use forall_protocol::models::PermissionProfile;
use forall_utils_absolute_path::AbsolutePathBuf;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

/// Kill switch for the elevated sandbox NUX on Windows.
///
/// When false, revert to the previous sandbox NUX, which only
/// prompts users to enable the legacy sandbox feature.
pub const ELEVATED_SANDBOX_NUX_ENABLED: bool = true;

pub trait WindowsSandboxLevelExt {
    fn from_config(config: &Config) -> WindowsSandboxLevel;
    fn from_features(features: &Features) -> WindowsSandboxLevel;
}

impl WindowsSandboxLevelExt for WindowsSandboxLevel {
    fn from_config(config: &Config) -> WindowsSandboxLevel {
        match config.permissions.windows_sandbox_mode {
            Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
            Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
            None => Self::from_features(&config.features),
        }
    }

    fn from_features(features: &Features) -> WindowsSandboxLevel {
        if features.enabled(Feature::WindowsSandboxElevated) {
            return WindowsSandboxLevel::Elevated;
        }
        if features.enabled(Feature::WindowsSandbox) {
            WindowsSandboxLevel::RestrictedToken
        } else {
            WindowsSandboxLevel::Disabled
        }
    }
}

pub fn windows_sandbox_level_from_config(config: &Config) -> WindowsSandboxLevel {
    WindowsSandboxLevel::from_config(config)
}

pub fn windows_sandbox_level_from_features(features: &Features) -> WindowsSandboxLevel {
    WindowsSandboxLevel::from_features(features)
}

pub fn resolve_windows_sandbox_mode(cfg: &ConfigToml) -> Option<WindowsSandboxModeToml> {
    cfg.windows
        .as_ref()
        .and_then(|windows| windows.sandbox)
        .or_else(|| legacy_windows_sandbox_mode(cfg.features.as_ref()))
}

pub fn resolve_windows_sandbox_private_desktop(cfg: &ConfigToml) -> bool {
    cfg.windows
        .as_ref()
        .and_then(|windows| windows.sandbox_private_desktop)
        .unwrap_or(true)
}

pub fn legacy_windows_sandbox_mode(
    features: Option<&FeaturesToml>,
) -> Option<WindowsSandboxModeToml> {
    let entries = features.map(FeaturesToml::entries)?;
    legacy_windows_sandbox_mode_from_entries(&entries)
}

pub fn legacy_windows_sandbox_mode_from_entries(
    entries: &BTreeMap<String, bool>,
) -> Option<WindowsSandboxModeToml> {
    if entries
        .get(Feature::WindowsSandboxElevated.key())
        .copied()
        .unwrap_or(false)
    {
        return Some(WindowsSandboxModeToml::Elevated);
    }
    if entries
        .get(Feature::WindowsSandbox.key())
        .copied()
        .unwrap_or(false)
        || entries
            .get("enable_experimental_windows_sandbox")
            .copied()
            .unwrap_or(false)
    {
        Some(WindowsSandboxModeToml::Unelevated)
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
pub fn sandbox_setup_is_complete(forall_home: &Path) -> bool {
    forall_windows_sandbox::sandbox_setup_is_complete(forall_home)
}

#[cfg(not(target_os = "windows"))]
pub fn sandbox_setup_is_complete(_forall_home: &Path) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn elevated_setup_failure_details(err: &anyhow::Error) -> Option<(String, String)> {
    let failure = forall_windows_sandbox::extract_setup_failure(err)?;
    let code = failure.code.as_str().to_string();
    let message = forall_windows_sandbox::sanitize_setup_metric_tag_value(&failure.message);
    Some((code, message))
}

#[cfg(not(target_os = "windows"))]
pub fn elevated_setup_failure_details(_err: &anyhow::Error) -> Option<(String, String)> {
    None
}

#[cfg(target_os = "windows")]
pub fn elevated_setup_failure_metric_name(err: &anyhow::Error) -> &'static str {
    if forall_windows_sandbox::extract_setup_failure(err).is_some_and(|failure| {
        matches!(
            failure.code,
            forall_windows_sandbox::SetupErrorCode::OrchestratorHelperLaunchCanceled
        )
    }) {
        "forall.windows_sandbox.elevated_setup_canceled"
    } else {
        "forall.windows_sandbox.elevated_setup_failure"
    }
}

#[cfg(not(target_os = "windows"))]
pub fn elevated_setup_failure_metric_name(_err: &anyhow::Error) -> &'static str {
    panic!("elevated_setup_failure_metric_name is only supported on Windows")
}

#[cfg(target_os = "windows")]
pub fn run_elevated_setup(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    forall_home: &Path,
) -> anyhow::Result<()> {
    let permissions =
        forall_windows_sandbox::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
            permission_profile,
            workspace_roots,
        )?;
    forall_windows_sandbox::run_elevated_setup(
        forall_windows_sandbox::SandboxSetupRequest {
            permissions: &permissions,
            command_cwd,
            env_map,
            forall_home,
            proxy_enforced: false,
        },
        forall_windows_sandbox::SetupRootOverrides::default(),
    )
}

#[cfg(target_os = "windows")]
pub fn run_elevated_provisioning_setup(forall_home: &Path, real_user: &str) -> anyhow::Result<()> {
    forall_windows_sandbox::run_elevated_provisioning_setup(forall_home, real_user)
}

#[cfg(not(target_os = "windows"))]
pub fn run_elevated_setup(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _forall_home: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("elevated Windows sandbox setup is only supported on Windows")
}

#[cfg(not(target_os = "windows"))]
pub fn run_elevated_provisioning_setup(
    _forall_home: &Path,
    _real_user: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("elevated Windows sandbox setup is only supported on Windows")
}

#[cfg(target_os = "windows")]
pub fn run_legacy_setup_preflight(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    forall_home: &Path,
) -> anyhow::Result<()> {
    forall_windows_sandbox::run_windows_sandbox_legacy_preflight(
        permission_profile,
        workspace_roots,
        forall_home,
        command_cwd,
        env_map,
    )
}

#[cfg(target_os = "windows")]
pub fn run_setup_refresh_with_extra_read_roots(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    forall_home: &Path,
    extra_read_roots: Vec<PathBuf>,
) -> anyhow::Result<()> {
    forall_windows_sandbox::run_setup_refresh_with_extra_read_roots(
        permission_profile,
        workspace_roots,
        command_cwd,
        env_map,
        forall_home,
        extra_read_roots,
        /*proxy_enforced*/ false,
    )
}

#[cfg(not(target_os = "windows"))]
pub fn run_legacy_setup_preflight(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _forall_home: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("legacy Windows sandbox setup is only supported on Windows")
}

#[cfg(not(target_os = "windows"))]
pub fn run_setup_refresh_with_extra_read_roots(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _forall_home: &Path,
    _extra_read_roots: Vec<PathBuf>,
) -> anyhow::Result<()> {
    anyhow::bail!("Windows sandbox read-root refresh is only supported on Windows")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsSandboxSetupMode {
    Elevated,
    Unelevated,
}

#[derive(Debug, Clone)]
pub struct WindowsSandboxSetupRequest {
    pub mode: WindowsSandboxSetupMode,
    pub permission_profile: PermissionProfile,
    pub workspace_roots: Vec<AbsolutePathBuf>,
    pub command_cwd: PathBuf,
    pub env_map: HashMap<String, String>,
    pub forall_home: PathBuf,
}

pub async fn run_windows_sandbox_setup(request: WindowsSandboxSetupRequest) -> anyhow::Result<()> {
    let start = Instant::now();
    let mode = request.mode;
    let originator_tag = sanitize_metric_tag_value(originator().value.as_str());
    let result = run_windows_sandbox_setup_and_persist(request).await;

    match result {
        Ok(()) => {
            emit_windows_sandbox_setup_success_metrics(
                mode,
                originator_tag.as_str(),
                start.elapsed(),
            );
            Ok(())
        }
        Err(err) => {
            emit_windows_sandbox_setup_failure_metrics(
                mode,
                originator_tag.as_str(),
                start.elapsed(),
                &err,
            );
            Err(err)
        }
    }
}

async fn run_windows_sandbox_setup_and_persist(
    request: WindowsSandboxSetupRequest,
) -> anyhow::Result<()> {
    let mode = request.mode;
    let permission_profile = request.permission_profile;
    let workspace_roots = request.workspace_roots;
    let command_cwd = request.command_cwd;
    let env_map = request.env_map;
    let forall_home = request.forall_home;
    let setup_forall_home = forall_home.clone();

    let setup_result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        match mode {
            WindowsSandboxSetupMode::Elevated => {
                if !sandbox_setup_is_complete(setup_forall_home.as_path()) {
                    run_elevated_setup(
                        &permission_profile,
                        workspace_roots.as_slice(),
                        command_cwd.as_path(),
                        &env_map,
                        setup_forall_home.as_path(),
                    )?;
                }
            }
            WindowsSandboxSetupMode::Unelevated => {
                run_legacy_setup_preflight(
                    &permission_profile,
                    workspace_roots.as_slice(),
                    command_cwd.as_path(),
                    &env_map,
                    setup_forall_home.as_path(),
                )?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("windows sandbox setup task failed: {join_err}"))?;

    setup_result?;

    ConfigEditsBuilder::new(forall_home.as_path())
        .set_windows_sandbox_mode(windows_sandbox_setup_mode_tag(mode))
        .clear_legacy_windows_sandbox_keys()
        .apply()
        .await
        .map_err(|err| anyhow::anyhow!("failed to persist windows sandbox mode: {err}"))
}

fn emit_windows_sandbox_setup_success_metrics(
    mode: WindowsSandboxSetupMode,
    originator_tag: &str,
    duration: std::time::Duration,
) {
    let Some(metrics) = forall_otel::global() else {
        return;
    };
    let mode_tag = windows_sandbox_setup_mode_tag(mode);
    let _ = metrics.record_duration(
        "forall.windows_sandbox.setup_duration_ms",
        duration,
        &[
            ("result", "success"),
            ("originator", originator_tag),
            ("mode", mode_tag),
        ],
    );
    let _ = metrics.counter(
        "forall.windows_sandbox.setup_success",
        /*inc*/ 1,
        &[("originator", originator_tag), ("mode", mode_tag)],
    );
}

fn emit_windows_sandbox_setup_failure_metrics(
    mode: WindowsSandboxSetupMode,
    originator_tag: &str,
    duration: std::time::Duration,
    _err: &anyhow::Error,
) {
    let Some(metrics) = forall_otel::global() else {
        return;
    };
    let mode_tag = windows_sandbox_setup_mode_tag(mode);
    let _ = metrics.record_duration(
        "forall.windows_sandbox.setup_duration_ms",
        duration,
        &[
            ("result", "failure"),
            ("originator", originator_tag),
            ("mode", mode_tag),
        ],
    );
    let _ = metrics.counter(
        "forall.windows_sandbox.setup_failure",
        /*inc*/ 1,
        &[("originator", originator_tag), ("mode", mode_tag)],
    );

    if matches!(mode, WindowsSandboxSetupMode::Elevated) {
        #[cfg(target_os = "windows")]
        {
            let mut failure_tags: Vec<(&str, &str)> = vec![("originator", originator_tag)];
            let mut code_tag: Option<String> = None;
            let mut message_tag: Option<String> = None;
            if let Some((code, message)) = elevated_setup_failure_details(_err) {
                code_tag = Some(code);
                message_tag = Some(message);
            }
            if let Some(code) = code_tag.as_deref() {
                failure_tags.push(("code", code));
            }
            if let Some(message) = message_tag.as_deref() {
                failure_tags.push(("message", message));
            }
            let _ = metrics.counter(
                elevated_setup_failure_metric_name(_err),
                /*inc*/ 1,
                &failure_tags,
            );
        }
    } else {
        let _ = metrics.counter(
            "forall.windows_sandbox.legacy_setup_preflight_failed",
            /*inc*/ 1,
            &[("originator", originator_tag)],
        );
    }
}

fn windows_sandbox_setup_mode_tag(mode: WindowsSandboxSetupMode) -> &'static str {
    match mode {
        WindowsSandboxSetupMode::Elevated => "elevated",
        WindowsSandboxSetupMode::Unelevated => "unelevated",
    }
}

#[cfg(test)]
#[path = "windows_sandbox_tests.rs"]
mod tests;
