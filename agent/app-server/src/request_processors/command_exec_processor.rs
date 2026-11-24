use super::*;

#[derive(Clone)]
pub(crate) struct CommandExecRequestProcessor {
    arg0_paths: Arg0DispatchPaths,
    config: Arc<Config>,
    outgoing: Arc<OutgoingMessageSender>,
    config_manager: ConfigManager,
    environment_manager: Arc<EnvironmentManager>,
    command_exec_manager: CommandExecManager,
}

impl CommandExecRequestProcessor {
    pub(crate) fn new(
        arg0_paths: Arg0DispatchPaths,
        config: Arc<Config>,
        outgoing: Arc<OutgoingMessageSender>,
        config_manager: ConfigManager,
        environment_manager: Arc<EnvironmentManager>,
    ) -> Self {
        Self {
            arg0_paths,
            config,
            outgoing,
            config_manager,
            environment_manager,
            command_exec_manager: CommandExecManager::default(),
        }
    }

    pub(crate) async fn one_off_command_exec(
        &self,
        request_id: &ConnectionRequestId,
        params: CommandExecParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.require_local_environment()?;
        self.exec_one_off_command(request_id, params)
            .await
            .map(|()| None)
    }

    pub(crate) async fn command_exec_write(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecWriteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.command_exec_manager
            .write(request_id, params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn command_exec_resize(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecResizeParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.command_exec_manager
            .resize(request_id, params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn command_exec_terminate(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecTerminateParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.command_exec_manager
            .terminate(request_id, params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn connection_closed(&self, connection_id: ConnectionId) {
        self.command_exec_manager
            .connection_closed(connection_id)
            .await;
    }

    fn require_local_environment(&self) -> Result<(), JSONRPCErrorError> {
        self.environment_manager
            .try_local_environment()
            .is_some()
            .then_some(())
            .ok_or_else(|| internal_error("local environment is not configured"))
    }

    async fn exec_one_off_command(
        &self,
        request_id: &ConnectionRequestId,
        params: CommandExecParams,
    ) -> Result<(), JSONRPCErrorError> {
        self.exec_one_off_command_inner(request_id.clone(), params)
            .await
    }

    async fn exec_one_off_command_inner(
        &self,
        request_id: ConnectionRequestId,
        params: CommandExecParams,
    ) -> Result<(), JSONRPCErrorError> {
        tracing::debug!("ExecOneOffCommand params: {params:?}");

        let request = request_id.clone();

        if params.command.is_empty() {
            return Err(invalid_request("command must not be empty"));
        }

        let CommandExecParams {
            command,
            process_id,
            tty,
            stream_stdin,
            stream_stdout_stderr,
            output_bytes_cap,
            disable_output_cap,
            disable_timeout,
            timeout_ms,
            cwd,
            env: env_overrides,
            size,
            sandbox_policy,
            permission_profile,
        } = params;
        if sandbox_policy.is_some() && permission_profile.is_some() {
            return Err(invalid_request(
                "`permissionProfile` cannot be combined with `sandboxPolicy`",
            ));
        }

        if size.is_some() && !tty {
            return Err(invalid_params("command/exec size requires tty: true"));
        }

        if disable_output_cap && output_bytes_cap.is_some() {
            return Err(invalid_params(
                "command/exec cannot set both outputBytesCap and disableOutputCap",
            ));
        }

        if disable_timeout && timeout_ms.is_some() {
            return Err(invalid_params(
                "command/exec cannot set both timeoutMs and disableTimeout",
            ));
        }

        let cwd = cwd.map_or_else(|| self.config.cwd.clone(), |cwd| self.config.cwd.join(cwd));
        let mut env = create_env(
            &self.config.permissions.shell_environment_policy,
            /*thread_id*/ None,
        );
        if let Some(env_overrides) = env_overrides {
            for (key, value) in env_overrides {
                match value {
                    Some(value) => {
                        env.insert(key, value);
                    }
                    None => {
                        env.remove(&key);
                    }
                }
            }
        }
        self.maybe_inject_github_git_credentials(&command, &mut env);
        let timeout_ms = match timeout_ms {
            Some(timeout_ms) => match u64::try_from(timeout_ms) {
                Ok(timeout_ms) => Some(timeout_ms),
                Err(_) => {
                    return Err(invalid_params(format!(
                        "command/exec timeoutMs must be non-negative, got {timeout_ms}"
                    )));
                }
            },
            None => None,
        };
        let windows_sandbox_level = WindowsSandboxLevel::from_config(&self.config);
        let output_bytes_cap = if disable_output_cap {
            None
        } else {
            Some(output_bytes_cap.unwrap_or(DEFAULT_OUTPUT_BYTES_CAP))
        };
        let expiration = if disable_timeout {
            ExecExpiration::Cancellation(CancellationToken::new())
        } else {
            match timeout_ms {
                Some(timeout_ms) => timeout_ms.into(),
                None => ExecExpiration::DefaultTimeout,
            }
        };
        let capture_policy = if disable_output_cap {
            ExecCapturePolicy::FullBuffer
        } else {
            ExecCapturePolicy::ShellTool
        };
        let sandbox_cwd = if permission_profile.is_some() {
            cwd.clone()
        } else {
            self.config.cwd.clone()
        };
        let (
            effective_permission_profile,
            network_proxy_spec,
            network_proxy_permission_profile,
            managed_network_requirements_enabled,
            windows_sandbox_workspace_roots,
        ) = if let Some(permission_profile) = permission_profile {
            let overrides = ConfigOverrides {
                cwd: Some(cwd.to_path_buf()),
                default_permissions: Some(permission_profile),
                ..Default::default()
            };
            let config = self
                .config_manager
                .load_for_cwd(
                    /*request_overrides*/ None,
                    overrides,
                    Some(self.config.cwd.to_path_buf()),
                )
                .await
                .map_err(|err| invalid_request(format!("invalid permission profile: {err}")))?;
            if let Some(warning) = config.startup_warnings.iter().find(|warning| {
                warning.contains("Configured value for `permission_profile` is disallowed")
            }) {
                return Err(invalid_request(format!(
                    "invalid permission profile: {warning}"
                )));
            }
            (
                config.permissions.effective_permission_profile(),
                config.permissions.network.clone(),
                config.permissions.permission_profile().clone(),
                config.managed_network_requirements_enabled(),
                config.effective_workspace_roots(),
            )
        } else if let Some(policy) = sandbox_policy.map(|policy| policy.to_core()) {
            self.config
                .permissions
                .can_set_legacy_sandbox_policy(&policy, &sandbox_cwd)
                .map_err(|err| invalid_request(format!("invalid sandbox policy: {err}")))?;
            let file_system_sandbox_policy =
                forall_protocol::permissions::FileSystemSandboxPolicy::from_legacy_sandbox_policy_for_cwd(&policy, &sandbox_cwd);
            let network_sandbox_policy =
                forall_protocol::permissions::NetworkSandboxPolicy::from(&policy);
            let permission_profile =
                forall_protocol::models::PermissionProfile::from_runtime_permissions_with_enforcement(
                    forall_protocol::models::SandboxEnforcement::from_legacy_sandbox_policy(&policy),
                    &file_system_sandbox_policy,
                    network_sandbox_policy,
                );
            self.config
                .permissions
                .can_set_permission_profile(&permission_profile)
                .map_err(|err| invalid_request(format!("invalid sandbox policy: {err}")))?;
            (
                permission_profile,
                self.config.permissions.network.clone(),
                self.config.permissions.permission_profile().clone(),
                self.config.managed_network_requirements_enabled(),
                self.config.effective_workspace_roots(),
            )
        } else {
            (
                self.config.permissions.effective_permission_profile(),
                self.config.permissions.network.clone(),
                self.config.permissions.permission_profile().clone(),
                self.config.managed_network_requirements_enabled(),
                self.config.effective_workspace_roots(),
            )
        };
        let started_network_proxy = match network_proxy_spec.as_ref() {
            Some(spec) => match spec
                .start_proxy(
                    &network_proxy_permission_profile,
                    /*policy_decider*/ None,
                    /*blocked_request_observer*/ None,
                    managed_network_requirements_enabled,
                    NetworkProxyAuditMetadata::default(),
                )
                .await
            {
                Ok(started) => Some(started),
                Err(err) => {
                    return Err(internal_error(format!(
                        "failed to start managed network proxy: {err}"
                    )));
                }
            },
            None => None,
        };
        let exec_params = ExecParams {
            command,
            cwd: cwd.clone(),
            expiration,
            capture_policy,
            env,
            network: started_network_proxy
                .as_ref()
                .map(forall_core::config::StartedNetworkProxy::proxy),
            network_environment_id: None,
            sandbox_permissions: SandboxPermissions::UseDefault,
            windows_sandbox_level,
            windows_sandbox_private_desktop: self
                .config
                .permissions
                .windows_sandbox_private_desktop,
            justification: None,
            arg0: None,
        };

        let forall_linux_sandbox_exe = self.arg0_paths.forall_linux_sandbox_exe.clone();
        let outgoing = self.outgoing.clone();
        let request_for_task = request.clone();
        let started_network_proxy_for_task = started_network_proxy;
        let use_legacy_landlock = self.config.features.use_legacy_landlock();
        let size = match size.map(crate::command_exec::terminal_size_from_protocol) {
            Some(Ok(size)) => Some(size),
            Some(Err(error)) => return Err(error),
            None => None,
        };

        let exec_request = forall_core::exec::build_exec_request(
            exec_params,
            &effective_permission_profile,
            &sandbox_cwd,
            windows_sandbox_workspace_roots.as_slice(),
            &forall_linux_sandbox_exe,
            use_legacy_landlock,
        )
        .map_err(|err| internal_error(format!("exec failed: {err}")))?;
        self.command_exec_manager
            .start(StartCommandExecParams {
                outgoing,
                request_id: request_for_task,
                process_id,
                exec_request,
                started_network_proxy: started_network_proxy_for_task,
                tty,
                stream_stdin,
                stream_stdout_stderr,
                output_bytes_cap,
                size,
            })
            .await
    }

    /// When a GitHub connection exists and this command is `git`, install a
    /// credential helper for `github.com` over HTTPS so clone/fetch/push
    /// against the user's repositories just work. The helper is this very
    /// binary (`forall git-credential-github`): the token itself never
    /// enters git's environment or argv, so hooks, filters and other git
    /// children inherit nothing secret. A caller that supplies its own
    /// `GIT_CONFIG_COUNT` is left untouched.
    ///
    /// Only the bare program name `git` qualifies — resolved through the
    /// inherited PATH, which the workspace does not control. Any path form
    /// (`./git`, `/tmp/git`, `sub/git.exe`) could name a workspace-planted
    /// executable and must not get the credential config either.
    ///
    /// Accepted residual, shared with every git credential helper (`gh auth
    /// git-credential`, osxkeychain, git-credential-manager): code already
    /// executing unsandboxed as the user — a hook this git run spawns, or
    /// anything else on the machine — can invoke the helper itself and
    /// obtain the token. No parent-process check can close this: git runs
    /// both the legitimate credential fetch and the hook through the same
    /// shell, so the attacker's invocation chain is indistinguishable from
    /// the real one. The boundary here is the user account; shrinking the
    /// blast radius further means per-repository short-lived installation
    /// tokens (the GitHub App track), not a smarter helper.
    fn maybe_inject_github_git_credentials(
        &self,
        command: &[String],
        env: &mut HashMap<String, String>,
    ) {
        let is_bare_git = command
            .first()
            .is_some_and(|program| is_trusted_bare_git(program));
        if !is_bare_git || env.contains_key("GIT_CONFIG_COUNT") {
            return;
        }
        match load_github_connection(
            &self.config.forall_home,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        ) {
            Ok(Some(_)) => {}
            Ok(None) => return,
            Err(err) => {
                tracing::warn!("failed to read GitHub connection for git credentials: {err}");
                return;
            }
        }
        let Ok(current_exe) = std::env::current_exe() else {
            tracing::warn!("cannot locate the forall binary for the git credential helper");
            return;
        };
        inject_github_credential_config(env, &current_exe.to_string_lossy());
    }
}

/// True only for the bare program name `git`, which the OS resolves through
/// the inherited PATH. Path forms may name workspace-controlled executables
/// and never qualify for credential injection.
fn is_trusted_bare_git(program: &str) -> bool {
    program == "git"
}

/// Configure git (via `GIT_CONFIG_*` environment entries, which touch no
/// file on disk) to authenticate `https://github.com` through
/// `forall git-credential-github`: first clear any inherited helpers for
/// that host so the connection wins over stale keychain entries, then point
/// git at this binary. No secret material appears in these entries.
fn inject_github_credential_config(env: &mut HashMap<String, String>, forall_exe: &str) {
    let entries = [
        ("credential.https://github.com.helper", String::new()),
        (
            "credential.https://github.com.helper",
            format!("!{} git-credential-github", shell_single_quote(forall_exe)),
        ),
    ];
    env.insert("GIT_CONFIG_COUNT".to_string(), entries.len().to_string());
    for (index, (key, value)) in entries.into_iter().enumerate() {
        env.insert(format!("GIT_CONFIG_KEY_{index}"), key.to_string());
        env.insert(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    // Never fall back to an interactive username/password prompt; without a
    // terminal that is a hang or a confusing failure.
    env.entry("GIT_TERMINAL_PROMPT".to_string())
        .or_insert_with(|| "0".to_string());
}

/// Single-quote a path for the POSIX shell git runs helpers with.
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod github_credentials_tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn credential_config_clears_inherited_helpers_then_installs_ours() {
        let mut env = HashMap::new();
        inject_github_credential_config(&mut env, "/opt/for all/forall");
        assert_eq!(env.get("GIT_CONFIG_COUNT").unwrap(), "2");
        assert_eq!(
            env.get("GIT_CONFIG_KEY_0").unwrap(),
            "credential.https://github.com.helper"
        );
        assert_eq!(env.get("GIT_CONFIG_VALUE_0").unwrap(), "");
        assert_eq!(
            env.get("GIT_CONFIG_KEY_1").unwrap(),
            "credential.https://github.com.helper"
        );
        // The helper is our own binary; no secret material is present in the
        // environment git (and every hook it spawns) inherits.
        assert_eq!(
            env.get("GIT_CONFIG_VALUE_1").unwrap(),
            "!'/opt/for all/forall' git-credential-github"
        );
        assert!(!env.contains_key("FORALL_GITHUB_TOKEN"));
        assert_eq!(env.get("GIT_TERMINAL_PROMPT").unwrap(), "0");
    }

    #[test]
    fn helper_path_single_quoting_survives_quotes() {
        assert_eq!(shell_single_quote("/a/b"), "'/a/b'");
        assert_eq!(shell_single_quote("/a'b"), "'/a'\\''b'");
    }

    /// The trust boundary is the bare name resolved through PATH: a
    /// workspace-planted executable that merely *looks like* git must never
    /// receive the repository token.
    #[test]
    fn only_the_bare_git_program_name_qualifies_for_injection() {
        assert!(is_trusted_bare_git("git"));
        for path_form in [
            "./git",
            "/tmp/git",
            "sub/git",
            "sub\\git.exe",
            "git.exe",
            "Git",
            "",
        ] {
            assert!(
                !is_trusted_bare_git(path_form),
                "{path_form:?} must not receive the token"
            );
        }
    }

    #[test]
    fn caller_terminal_prompt_setting_is_respected() {
        let mut env = HashMap::from([("GIT_TERMINAL_PROMPT".to_string(), "1".to_string())]);
        inject_github_credential_config(&mut env, "/usr/local/bin/forall");
        assert_eq!(env.get("GIT_TERMINAL_PROMPT").unwrap(), "1");
    }
}
