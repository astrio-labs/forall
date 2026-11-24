use super::*;

mod hosted_api;
mod rate_limit_resets;

const HOSTED_BACKEND_UNAVAILABLE: &str =
    "Hosted Forall backend features are not available in BYOK builds.";

fn hosted_backend_unavailable() -> JSONRPCErrorError {
    invalid_request(HOSTED_BACKEND_UNAVAILABLE)
}

enum RefreshTokenRequestOutcome {
    NotAttemptedOrSucceeded,
    FailedTransiently,
    FailedPermanently,
}

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config: Arc<Config>,
    config_manager: ConfigManager,
    /// In-flight OAuth logins keyed by the `login_id` returned from
    /// `account/login/start`; shared across clones of the processor.
    pending_oauth_logins: Arc<Mutex<HashMap<String, CancelHandle>>>,
    /// In-flight GitHub connect flows keyed by the `connect_id` returned
    /// from `account/github/connect/start`.
    pending_github_connects: Arc<Mutex<HashMap<String, CancelHandle>>>,
}

impl AccountRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config: Arc<Config>,
        config_manager: ConfigManager,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config,
            config_manager,
            pending_oauth_logins: Arc::new(Mutex::new(HashMap::new())),
            pending_github_connects: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn login_account(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.login_v2(request_id, params).await.map(|()| None)
    }

    pub(crate) async fn logout_account(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.logout_v2(request_id).await.map(|()| None)
    }

    pub(crate) async fn cancel_login_account(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.cancel_login_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account(
        &self,
        params: GetAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_auth_status(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_auth_status_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account_rate_limits(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_rate_limits_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account_token_usage(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_token_usage_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_workspace_messages(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_workspace_messages_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn send_add_credits_nudge_email(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.send_add_credits_nudge_email_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn cancel_active_login(&self) {
        let handles: Vec<CancelHandle> = {
            let mut pending = self.pending_oauth_logins.lock().await;
            pending.drain().map(|(_, handle)| handle).collect()
        };
        for handle in handles {
            handle.cancel();
        }
        self.cancel_active_github_connects().await;
    }

    /// Cancel every pending GitHub connect flow. `CancelHandle::cancel`
    /// serializes against the worker's save, so once this returns no flow
    /// can persist a connection — which is why logout and disconnect call it
    /// BEFORE wiping credentials: a browser authorization completing a
    /// moment later must not resurrect what the user just removed.
    async fn cancel_active_github_connects(&self) {
        let connects: Vec<CancelHandle> = {
            let mut pending = self.pending_github_connects.lock().await;
            pending.drain().map(|(_, handle)| handle).collect()
        };
        for handle in connects {
            handle.cancel();
        }
    }

    pub(crate) fn clear_external_auth(&self) {
        self.auth_manager.clear_external_auth();
        self.thread_manager
            .plugins_manager()
            .set_auth_mode(self.auth_manager.get_api_auth_mode());
    }

    fn current_account_updated_notification(&self) -> AccountUpdatedNotification {
        let auth = self.auth_manager.auth_cached();
        AccountUpdatedNotification {
            auth_mode: auth.as_ref().map(ForallAuth::api_auth_mode),
            plan_type: auth.as_ref().and_then(ForallAuth::account_plan_type),
        }
    }

    async fn maybe_refresh_plugin_caches_for_current_config(
        config_manager: &ConfigManager,
        thread_manager: &Arc<ThreadManager>,
        auth: Option<ForallAuth>,
    ) {
        thread_manager
            .plugins_manager()
            .set_auth_mode(auth.as_ref().map(ForallAuth::api_auth_mode));
        thread_manager
            .plugins_manager()
            .clear_recommended_plugins_cache();

        match config_manager
            .load_latest_config(/*fallback_cwd*/ None)
            .await
        {
            Ok(config) => {
                let refresh_thread_manager = Arc::clone(thread_manager);
                let refresh_config_manager = config_manager.clone();
                thread_manager
                    .plugins_manager()
                    .maybe_start_remote_plugin_caches_refresh(
                        &config.plugins_config_input(),
                        auth,
                        Some(Arc::new(move || {
                            Self::spawn_effective_plugins_changed_task(
                                Arc::clone(&refresh_thread_manager),
                                refresh_config_manager.clone(),
                            );
                        })),
                    );
            }
            Err(err) => {
                warn!(
                    "failed to reload config after account changed, skipping remote installed plugins cache refresh: {err}"
                );
            }
        }
    }

    fn spawn_effective_plugins_changed_task(
        thread_manager: Arc<ThreadManager>,
        config_manager: ConfigManager,
    ) {
        tokio::spawn(async move {
            thread_manager.plugins_manager().clear_cache();
            thread_manager.skills_service().clear_cache();
            if thread_manager.list_thread_ids().await.is_empty() {
                return;
            }
            crate::mcp_refresh::queue_best_effort_refresh(&thread_manager, &config_manager).await;
        });
    }

    async fn login_v2(
        &self,
        request_id: ConnectionRequestId,
        params: LoginAccountParams,
    ) -> Result<(), JSONRPCErrorError> {
        match params {
            LoginAccountParams::ApiKey {
                api_key,
                model_provider,
            } => {
                self.login_api_key_v2(
                    request_id,
                    LoginApiKeyParams {
                        api_key,
                        model_provider,
                    },
                )
                .await;
            }
            LoginAccountParams::Oauth { provider } => {
                self.login_oauth_v2(request_id, provider).await;
            }
        }
        Ok(())
    }

    /// Starts a managed Forall OAuth login: binds the loopback server, responds with the
    /// authorization URL (the client opens the browser), and reports the outcome via
    /// `account/login/completed`.
    async fn login_oauth_v2(
        &self,
        request_id: ConnectionRequestId,
        provider: Option<OauthSignInProvider>,
    ) {
        let base_urls = AuthBaseUrls::resolve(self.config.forall_auth_base_url.as_deref());
        let mut options = OauthLoginOptions::new(
            base_urls,
            self.config.forall_home.to_path_buf(),
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        );
        options.provider = match provider {
            // The auth service's default is Google; an explicit Google
            // request maps to the same thing.
            None | Some(OauthSignInProvider::Google) => OauthLoginProvider::Default,
            Some(OauthSignInProvider::Github) => OauthLoginProvider::Github,
        };
        // The client (TUI, IDE, ...) is responsible for opening `auth_url`.
        options.open_browser = false;

        let started = match OauthLoginFlow::start(options) {
            Ok(started) => started,
            Err(err) => {
                let result: Result<LoginAccountResponse, JSONRPCErrorError> = Err(internal_error(
                    format!("failed to start oauth login: {err}"),
                ));
                self.outgoing.send_result(request_id, result).await;
                return;
            }
        };

        let login_uuid = Uuid::new_v4();
        let login_id = login_uuid.to_string();
        {
            let mut pending = self.pending_oauth_logins.lock().await;
            pending.insert(login_id.clone(), started.cancel.clone());
        }

        let result: Result<LoginAccountResponse, JSONRPCErrorError> =
            Ok(LoginAccountResponse::Oauth {
                login_id: login_id.clone(),
                auth_url: started.auth_url,
            });
        self.outgoing.send_result(request_id, result).await;

        let processor = self.clone();
        let completion = started.completion;
        tokio::spawn(async move {
            let result = completion.await;
            {
                let mut pending = processor.pending_oauth_logins.lock().await;
                pending.remove(&login_id);
            }
            match result {
                Ok(Ok(_outcome)) => {
                    processor.auth_manager.reload().await;
                    // Mirror the apiKey path, which persists the provider the
                    // key belongs to: a Forall sign-in selects the hosted
                    // `astrio` provider. The Forall token is only valid at
                    // the gateway, so leaving the previous provider active
                    // would send the next turn to a host that rejects it.
                    if let Err(err) = processor.persist_astrio_model_provider().await {
                        tracing::warn!(
                            "signed in, but failed to select the astrio provider: {err:?}"
                        );
                    }
                    processor
                        .send_login_success_notifications(Some(login_uuid))
                        .await;
                }
                Ok(Err(err)) => {
                    processor
                        .send_login_failed_notification(login_id, err.to_string())
                        .await;
                }
                Err(_) => {
                    processor
                        .send_login_failed_notification(
                            login_id,
                            "the login flow terminated unexpectedly".to_string(),
                        )
                        .await;
                }
            }
        });
    }

    async fn send_login_failed_notification(&self, login_id: String, error: String) {
        self.outgoing
            .send_server_notification(ServerNotification::AccountLoginCompleted(
                AccountLoginCompletedNotification {
                    login_id: Some(login_id),
                    success: false,
                    error: Some(error),
                },
            ))
            .await;
    }

    async fn login_api_key_common(
        &self,
        params: &LoginApiKeyParams,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        match set_byok_api_key(
            &self.config.forall_home,
            params.model_provider.as_deref().unwrap_or("openai"),
            &params.api_key,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        ) {
            Ok(()) => {
                self.auth_manager.reload().await;
                if let Some(provider) = params.model_provider.as_deref() {
                    self.persist_user_model_provider(provider).await?;
                }
                Ok(())
            }
            Err(err) => Err(internal_error(format!("failed to save api key: {err}"))),
        }
    }

    /// After a Forall sign-in, chat runs through the hosted gateway: select
    /// the built-in `astrio` provider and clear the model override so the
    /// engine default applies. `byok_provider` is left alone — it remembers
    /// the user's last BYOK choice for when they switch back.
    async fn persist_astrio_model_provider(&self) -> std::result::Result<(), JSONRPCErrorError> {
        use forall_app_server_protocol::ConfigBatchWriteParams;
        use forall_app_server_protocol::ConfigEdit;
        use forall_app_server_protocol::MergeStrategy;
        use serde_json::json;

        const ASTRIO_PROVIDER_ID: &str = "astrio";

        self.config_manager
            .batch_write(ConfigBatchWriteParams {
                edits: vec![
                    ConfigEdit {
                        key_path: "model_provider".to_string(),
                        value: json!(ASTRIO_PROVIDER_ID),
                        merge_strategy: MergeStrategy::Replace,
                    },
                    ConfigEdit {
                        key_path: "model".to_string(),
                        value: json!(null),
                        merge_strategy: MergeStrategy::Replace,
                    },
                ],
                file_path: None,
                expected_version: None,
                reload_user_config: true,
            })
            .await
            .map_err(|err| {
                internal_error(format!("failed to select the astrio provider: {err}"))
            })?;
        Ok(())
    }

    async fn persist_user_model_provider(
        &self,
        model_provider_id: &str,
    ) -> std::result::Result<(), JSONRPCErrorError> {
        use forall_app_server_protocol::ConfigBatchWriteParams;
        use forall_app_server_protocol::ConfigEdit;
        use forall_app_server_protocol::MergeStrategy;
        use serde_json::json;

        const OPENROUTER_PROVIDER_ID: &str = "openrouter";
        const OPENROUTER_DEFAULT_MODEL: &str = "openrouter/auto";

        let model_value = if model_provider_id == OPENROUTER_PROVIDER_ID {
            json!(OPENROUTER_DEFAULT_MODEL)
        } else {
            json!(null)
        };

        self.config_manager
            .batch_write(ConfigBatchWriteParams {
                edits: vec![
                    ConfigEdit {
                        key_path: "model_provider".to_string(),
                        value: json!(model_provider_id),
                        merge_strategy: MergeStrategy::Replace,
                    },
                    ConfigEdit {
                        key_path: "byok_provider".to_string(),
                        value: json!(model_provider_id),
                        merge_strategy: MergeStrategy::Replace,
                    },
                    ConfigEdit {
                        key_path: "model".to_string(),
                        value: model_value,
                        merge_strategy: MergeStrategy::Replace,
                    },
                ],
                file_path: None,
                expected_version: None,
                reload_user_config: true,
            })
            .await
            .map_err(|err| internal_error(format!("failed to save model provider: {err}")))?;
        Ok(())
    }

    async fn login_api_key_v2(&self, request_id: ConnectionRequestId, params: LoginApiKeyParams) {
        let result = self
            .login_api_key_common(&params)
            .await
            .map(|()| LoginAccountResponse::ApiKey {});
        let logged_in = result.is_ok();
        self.outgoing.send_result(request_id, result).await;

        if logged_in {
            self.send_login_success_notifications(/*login_id*/ None)
                .await;
        }
    }

    async fn cancel_login_response(
        &self,
        params: CancelLoginAccountParams,
    ) -> Result<CancelLoginAccountResponse, JSONRPCErrorError> {
        let handle = {
            let mut pending = self.pending_oauth_logins.lock().await;
            pending.remove(&params.login_id)
        };
        match handle {
            Some(handle) => {
                handle.cancel();
                Ok(CancelLoginAccountResponse {
                    status: CancelLoginAccountStatus::Canceled,
                })
            }
            None => Ok(CancelLoginAccountResponse {
                status: CancelLoginAccountStatus::NotFound,
            }),
        }
    }

    async fn send_login_success_notifications(&self, login_id: Option<Uuid>) {
        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        let payload_login_completed = AccountLoginCompletedNotification {
            login_id: login_id.map(|id| id.to_string()),
            success: true,
            error: None,
        };
        self.outgoing
            .send_server_notification(ServerNotification::AccountLoginCompleted(
                payload_login_completed,
            ))
            .await;

        self.outgoing
            .send_server_notification(ServerNotification::AccountUpdated(
                self.current_account_updated_notification(),
            ))
            .await;
    }

    async fn logout_common(&self) -> std::result::Result<Option<AuthMode>, JSONRPCErrorError> {
        // A connect flow completing after the wipe would recreate credential
        // storage; kill every pending flow first.
        self.cancel_active_github_connects().await;
        match self.auth_manager.logout_with_revoke().await {
            Ok(_) => {}
            Err(err) => {
                return Err(internal_error(format!("logout failed: {err}")));
            }
        }

        Self::maybe_refresh_plugin_caches_for_current_config(
            &self.config_manager,
            &self.thread_manager,
            self.auth_manager.auth_cached(),
        )
        .await;

        // Reflect the current auth method after logout (likely None).
        Ok(self
            .auth_manager
            .auth_cached()
            .as_ref()
            .map(ForallAuth::api_auth_mode))
    }

    async fn logout_v2(&self, request_id: ConnectionRequestId) -> Result<(), JSONRPCErrorError> {
        let result = self.logout_common().await;
        let account_updated =
            result
                .as_ref()
                .ok()
                .cloned()
                .map(|auth_mode| AccountUpdatedNotification {
                    auth_mode,
                    plan_type: None,
                });
        self.outgoing
            .send_result(request_id, result.map(|_| LogoutAccountResponse {}))
            .await;

        if let Some(payload) = account_updated {
            self.outgoing
                .send_server_notification(ServerNotification::AccountUpdated(payload))
                .await;
        }
        Ok(())
    }

    async fn refresh_token_if_requested(&self, do_refresh: bool) -> RefreshTokenRequestOutcome {
        if do_refresh && let Err(err) = self.auth_manager.refresh_token().await {
            let failed_reason = err.failed_reason();
            if failed_reason.is_none() {
                tracing::warn!("failed to refresh token while getting account: {err}");
                return RefreshTokenRequestOutcome::FailedTransiently;
            }
            return RefreshTokenRequestOutcome::FailedPermanently;
        }
        RefreshTokenRequestOutcome::NotAttemptedOrSucceeded
    }

    async fn get_auth_status_response(
        &self,
        params: GetAuthStatusParams,
    ) -> Result<GetAuthStatusResponse, JSONRPCErrorError> {
        let include_token = params.include_token.unwrap_or(false);
        let do_refresh = params.refresh_token.unwrap_or(false);

        self.refresh_token_if_requested(do_refresh).await;

        // Determine whether auth is required based on the active model provider.
        // If a custom provider is configured with `requires_openai_auth == false`,
        // then no auth step is required; otherwise, default to requiring auth.
        let requires_openai_auth = self.config.model_provider.requires_openai_auth;

        let response = if !requires_openai_auth {
            GetAuthStatusResponse {
                auth_method: None,
                auth_token: None,
                requires_openai_auth: Some(false),
            }
        } else {
            let auth = if do_refresh {
                self.auth_manager.auth_cached()
            } else {
                self.auth_manager.auth().await
            };
            match auth {
                Some(auth) => {
                    let permanent_refresh_failure =
                        self.auth_manager.refresh_failure_for_auth(&auth).is_some();
                    let auth_mode = auth.api_auth_mode();
                    let (reported_auth_method, token_opt) =
                        if include_token && permanent_refresh_failure {
                            // This response cannot represent the metadata needed to reuse these
                            // credentials.
                            (Some(auth_mode), None)
                        } else {
                            match auth.get_token() {
                                Ok(token) if !token.is_empty() => {
                                    let tok = if include_token { Some(token) } else { None };
                                    (Some(auth_mode), tok)
                                }
                                Ok(_) => (None, None),
                                Err(err) => {
                                    tracing::warn!("failed to get token for auth status: {err}");
                                    (None, None)
                                }
                            }
                        };
                    GetAuthStatusResponse {
                        auth_method: reported_auth_method,
                        auth_token: token_opt,
                        requires_openai_auth: Some(true),
                    }
                }
                None => GetAuthStatusResponse {
                    auth_method: None,
                    auth_token: None,
                    requires_openai_auth: Some(true),
                },
            }
        };

        Ok(response)
    }

    async fn get_account_response(
        &self,
        params: GetAccountParams,
    ) -> Result<GetAccountResponse, JSONRPCErrorError> {
        let do_refresh = params.refresh_token;

        self.refresh_token_if_requested(do_refresh).await;

        let provider = create_model_provider(
            self.config.model_provider.clone(),
            Some(self.auth_manager.clone()),
        );
        let account_state = match provider.account_state() {
            Ok(account_state) => account_state,
            Err(err) => return Err(invalid_request(err.to_string())),
        };
        let account = account_state.account.map(Account::from);

        Ok(GetAccountResponse {
            account,
            requires_openai_auth: account_state.requires_openai_auth,
        })
    }

    async fn get_account_rate_limits_response(
        &self,
    ) -> Result<GetAccountRateLimitsResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "authentication required to read rate limits",
            ));
        };
        if !matches!(auth, ForallAuth::Oauth(_)) {
            return Err(hosted_backend_unavailable());
        }
        self.fetch_hosted_rate_limits(&auth).await
    }

    async fn get_account_token_usage_response(
        &self,
    ) -> Result<GetAccountTokenUsageResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "authentication required to read token usage",
            ));
        };
        if !matches!(auth, ForallAuth::Oauth(_)) {
            return Err(hosted_backend_unavailable());
        }
        self.fetch_hosted_token_usage(&auth).await
    }

    async fn get_workspace_messages_response(
        &self,
    ) -> Result<GetWorkspaceMessagesResponse, JSONRPCErrorError> {
        let Some(_auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "authentication required to read workspace messages",
            ));
        };
        Err(hosted_backend_unavailable())
    }

    async fn send_add_credits_nudge_email_response(
        &self,
        _params: SendAddCreditsNudgeEmailParams,
    ) -> Result<SendAddCreditsNudgeEmailResponse, JSONRPCErrorError> {
        let Some(_auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "authentication required to notify workspace owner",
            ));
        };
        Err(hosted_backend_unavailable())
    }

    /// `account/github/read` — current state of the GitHub connection.
    pub(crate) async fn get_github_account(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let connection = load_github_connection(
            &self.config.forall_home,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        )
        .map_err(|err| internal_error(format!("failed to read GitHub connection: {err}")))?;
        let response = match connection {
            Some(connection) => GithubAccountReadResponse {
                connected: true,
                login: Some(connection.login),
                scopes: connection.scopes,
            },
            None => GithubAccountReadResponse {
                connected: false,
                login: None,
                scopes: Vec::new(),
            },
        };
        Ok(Some(response.into()))
    }

    /// `account/github/repos/list` — repositories visible to the connection,
    /// most recently pushed first.
    pub(crate) async fn list_github_repositories(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let connection = load_github_connection(
            &self.config.forall_home,
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        )
        .map_err(|err| internal_error(format!("failed to read GitHub connection: {err}")))?;
        let Some(connection) = connection else {
            return Err(invalid_request("GitHub is not connected".to_string()));
        };
        let repositories = list_repositories(&connection, GITHUB_API_BASE)
            .await
            .map_err(|err| internal_error(format!("failed to list GitHub repositories: {err}")))?;
        let response = GithubRepositoriesListResponse {
            repositories: repositories
                .into_iter()
                .map(|repo| GithubRepository {
                    full_name: repo.full_name,
                    name: repo.name,
                    owner: repo.owner,
                    private: repo.private,
                    clone_url: repo.clone_url,
                    default_branch: repo.default_branch,
                    pushed_at: repo.pushed_at,
                })
                .collect(),
        };
        Ok(Some(response.into()))
    }

    /// `account/github/connect/start` — begin the browser flow that connects
    /// a GitHub account for repository access. The outcome arrives as an
    /// `account/github/connect/completed` notification.
    pub(crate) async fn connect_github_account(
        &self,
        request_id: ConnectionRequestId,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        // The connect start call is authenticated with the Forall account
        // token. Refresh first (best-effort): the stored access token may
        // have outlived its hour.
        let auth = self.auth_manager.auth().await;
        if !matches!(auth, Some(ForallAuth::Oauth(_))) {
            let result: Result<GithubConnectStartResponse, JSONRPCErrorError> = Err(
                invalid_request("sign in with your Forall account before connecting GitHub"),
            );
            self.outgoing.send_result(request_id, result).await;
            return Ok(None);
        }
        if let Err(err) = self.auth_manager.refresh_token().await {
            tracing::warn!("token refresh before GitHub connect failed: {err}");
        }
        let Some(access_token) = self
            .auth_manager
            .auth()
            .await
            .and_then(|auth| auth.get_token().ok())
        else {
            let result: Result<GithubConnectStartResponse, JSONRPCErrorError> = Err(
                invalid_request("sign in with your Forall account before connecting GitHub"),
            );
            self.outgoing.send_result(request_id, result).await;
            return Ok(None);
        };

        let options = GithubConnectOptions {
            base_urls: AuthBaseUrls::resolve(self.config.forall_auth_base_url.as_deref()),
            account_access_token: access_token,
            forall_home: self.config.forall_home.to_path_buf(),
            store_mode: self.config.cli_auth_credentials_store_mode,
            keyring_kind: self.config.auth_keyring_backend_kind(),
            // The client (desktop, TUI, ...) opens `auth_url`.
            open_browser: false,
            timeout: forall_login::DEFAULT_CONNECT_TIMEOUT,
        };
        let started = match GithubConnectFlow::start(options).await {
            Ok(started) => started,
            Err(err) => {
                let result: Result<GithubConnectStartResponse, JSONRPCErrorError> = Err(
                    internal_error(format!("failed to start GitHub connect: {err}")),
                );
                self.outgoing.send_result(request_id, result).await;
                return Ok(None);
            }
        };

        let connect_id = Uuid::new_v4().to_string();
        {
            let mut pending = self.pending_github_connects.lock().await;
            pending.insert(connect_id.clone(), started.cancel.clone());
        }
        let result: Result<GithubConnectStartResponse, JSONRPCErrorError> =
            Ok(GithubConnectStartResponse {
                connect_id: connect_id.clone(),
                auth_url: started.auth_url,
            });
        self.outgoing.send_result(request_id, result).await;

        let processor = self.clone();
        let completion = started.completion;
        tokio::spawn(async move {
            let result = completion.await;
            {
                let mut pending = processor.pending_github_connects.lock().await;
                pending.remove(&connect_id);
            }
            let notification = match result {
                Ok(Ok(connection)) => GithubConnectCompletedNotification {
                    connect_id: Some(connect_id),
                    success: true,
                    error: None,
                    login: Some(connection.login),
                },
                Ok(Err(err)) => GithubConnectCompletedNotification {
                    connect_id: Some(connect_id),
                    success: false,
                    error: Some(err.to_string()),
                    login: None,
                },
                Err(_) => GithubConnectCompletedNotification {
                    connect_id: Some(connect_id),
                    success: false,
                    error: Some("the GitHub connect flow terminated unexpectedly".to_string()),
                    login: None,
                },
            };
            processor
                .outgoing
                .send_server_notification(ServerNotification::GithubConnectCompleted(notification))
                .await;
        });
        Ok(None)
    }

    pub(crate) async fn cancel_connect_github_account(
        &self,
        params: GithubConnectCancelParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let handle = {
            let mut pending = self.pending_github_connects.lock().await;
            pending.remove(&params.connect_id)
        };
        let status = match handle {
            Some(handle) => {
                handle.cancel();
                GithubConnectCancelStatus::Canceled
            }
            None => GithubConnectCancelStatus::NotFound,
        };
        Ok(Some(GithubConnectCancelResponse { status }.into()))
    }

    /// `account/github/disconnect` — best-effort upstream revocation, then
    /// forget the stored connection.
    pub(crate) async fn disconnect_github_account(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        // A pending connect completing after the disconnect would make the
        // connection reappear; kill every pending flow first.
        self.cancel_active_github_connects().await;
        let disconnected = disconnect_github(
            &self.config.forall_home,
            &AuthBaseUrls::resolve(self.config.forall_auth_base_url.as_deref()),
            self.config.cli_auth_credentials_store_mode,
            self.config.auth_keyring_backend_kind(),
        )
        .await
        .map_err(|err| internal_error(format!("failed to disconnect GitHub: {err}")))?;
        Ok(Some(GithubDisconnectResponse { disconnected }.into()))
    }
}
