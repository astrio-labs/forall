use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use forall_arg0::Arg0DispatchPaths;
use forall_core::ThreadManager;
use forall_core::config::ConfigOverrides;
use forall_external_agent_sessions::CompletedExternalAgentSessionImport;
use forall_external_agent_sessions::ExternalAgentSessionMigration;
use forall_external_agent_sessions::ImportedExternalAgentSession;
use forall_external_agent_sessions::PendingSessionImport;
use forall_external_agent_sessions::prepare_validated_session_import;
use forall_external_agent_sessions::record_completed_session_imports;
use forall_models_manager::manager::RefreshStrategy;
use forall_protocol::ThreadId;
use forall_protocol::models::BaseInstructions;
use forall_protocol::protocol::MultiAgentVersion;
use forall_protocol::protocol::ThreadMemoryMode;
use forall_rollout::is_persisted_rollout_item;
use forall_thread_store::AppendThreadItemsParams;
use forall_thread_store::CreateThreadParams;
use forall_thread_store::ThreadMetadataPatch;
use forall_thread_store::ThreadPersistenceMetadata;
use forall_thread_store::ThreadStore;
use forall_thread_store::UpdateThreadMetadataParams;
use futures::StreamExt;
use tokio::sync::Semaphore;

use crate::config::external_agent_config::ExternalAgentConfigImportItemResult;
use crate::config::external_agent_config::record_import_error;
use crate::config_manager::ConfigManager;

const SESSION_IMPORT_CONCURRENCY: usize = 5;

#[derive(Clone)]
pub(super) struct ExternalAgentSessionImporter {
    forall_home: PathBuf,
    permits: Arc<Semaphore>,
    thread_manager: Arc<ThreadManager>,
    thread_store: Arc<dyn ThreadStore>,
    config_manager: ConfigManager,
    arg0_paths: Arg0DispatchPaths,
}

impl ExternalAgentSessionImporter {
    pub(super) fn new(
        forall_home: PathBuf,
        thread_manager: Arc<ThreadManager>,
        thread_store: Arc<dyn ThreadStore>,
        config_manager: ConfigManager,
        arg0_paths: Arg0DispatchPaths,
    ) -> Self {
        Self {
            forall_home,
            permits: Arc::new(Semaphore::new(1)),
            thread_manager,
            thread_store,
            config_manager,
            arg0_paths,
        }
    }

    pub(super) async fn import_sessions(
        &self,
        sessions: Vec<ExternalAgentSessionMigration>,
        mut item_result: ExternalAgentConfigImportItemResult,
    ) -> ExternalAgentConfigImportItemResult {
        if sessions.is_empty() {
            return item_result;
        }
        let Ok(_permit) = self.permits.acquire().await else {
            record_import_error(
                &mut item_result,
                "session_permit",
                "external agent session import permit could not be acquired",
                /*source*/ None,
            );
            return item_result;
        };
        let import_results = futures::stream::iter(sessions)
            .map(|session| {
                let importer = self.clone();
                async move { importer.import_requested_session(session).await }
            })
            .buffer_unordered(SESSION_IMPORT_CONCURRENCY);
        futures::pin_mut!(import_results);

        let mut completed_imports = Vec::new();
        while let Some(result) = import_results.next().await {
            match result {
                Ok(Some(completed_import)) => {
                    item_result.record_success(
                        Some(completed_import.source_path.display().to_string()),
                        Some(completed_import.imported_thread_id.to_string()),
                    );
                    completed_imports.push(completed_import);
                }
                Ok(None) => {}
                Err(failure) => {
                    record_import_error(
                        &mut item_result,
                        failure.stage,
                        failure.message.clone(),
                        Some(failure.source_path.display().to_string()),
                    );
                }
            }
        }
        if let Err(err) = record_completed_session_imports(&self.forall_home, completed_imports) {
            record_import_error(
                &mut item_result,
                "session_ledger_update",
                err.to_string(),
                /*source*/ None,
            );
        }
        item_result
    }

    async fn import_requested_session(
        &self,
        session: ExternalAgentSessionMigration,
    ) -> Result<Option<CompletedExternalAgentSessionImport>, SessionImportFailure> {
        let source_path = session.path.clone();
        let Some(pending_import) =
            self.prepare_session_import(session)
                .await
                .map_err(|message| SessionImportFailure {
                    source_path: source_path.clone(),
                    message,
                    stage: "session_prepare",
                })?
        else {
            return Ok(None);
        };
        let imported_thread_id =
            self.persist_session(pending_import.session)
                .await
                .map_err(|message| SessionImportFailure {
                    source_path: pending_import.source_path.clone(),
                    message,
                    stage: "session_persist",
                })?;
        Ok(Some(CompletedExternalAgentSessionImport {
            source_path: pending_import.source_path,
            source_content_sha256: pending_import.source_content_sha256,
            imported_thread_id,
        }))
    }

    async fn prepare_session_import(
        &self,
        session: ExternalAgentSessionMigration,
    ) -> Result<Option<PendingSessionImport>, String> {
        let forall_home = self.forall_home.clone();
        tokio::task::spawn_blocking(move || prepare_validated_session_import(&forall_home, session))
            .await
            .map_err(|err| format!("external agent session preparation task failed: {err}"))?
            .map_err(|err| format!("failed to prepare external agent session: {err}"))
    }

    async fn persist_session(
        &self,
        session: ImportedExternalAgentSession,
    ) -> Result<ThreadId, String> {
        let ImportedExternalAgentSession {
            cwd,
            title,
            first_user_message,
            mut rollout_items,
        } = session;
        let config = self
            .config_manager
            .load_with_overrides(
                /*request_overrides*/ None,
                ConfigOverrides {
                    cwd: Some(cwd),
                    forall_linux_sandbox_exe: self.arg0_paths.forall_linux_sandbox_exe.clone(),
                    main_execve_wrapper_exe: self.arg0_paths.main_execve_wrapper_exe.clone(),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| format!("failed to load imported session config: {err}"))?;
        let models_manager = self.thread_manager.get_models_manager();
        let model = models_manager
            .get_default_model(&config.model, RefreshStrategy::Offline)
            .await;
        let model_info = models_manager
            .get_model_info(model.as_str(), &config.to_models_manager_config())
            .await;
        let thread_id = ThreadId::new();
        let source = self.thread_manager.session_source();
        let cwd = config.cwd.to_path_buf();
        let model_provider = config.model_provider_id.clone();
        let memory_mode = if config.memories.generate_memories {
            ThreadMemoryMode::Enabled
        } else {
            ThreadMemoryMode::Disabled
        };
        let now = Utc::now();
        let create_params = CreateThreadParams {
            session_id: thread_id.into(),
            thread_id,
            extra_config: None,
            forked_from_id: None,
            parent_thread_id: None,
            source: source.clone(),
            thread_source: None,
            base_instructions: BaseInstructions {
                text: config.base_instructions.clone().unwrap_or_else(|| {
                    model_info.get_model_instructions(/*personality*/ None)
                }),
            },
            dynamic_tools: Vec::new(),
            multi_agent_version: Some(MultiAgentVersion::V1),
            metadata: ThreadPersistenceMetadata {
                cwd: Some(cwd.clone()),
                model_provider: model_provider.clone(),
                memory_mode,
            },
        };
        rollout_items.retain(is_persisted_rollout_item);
        let title = title
            .as_deref()
            .and_then(forall_core::util::normalize_thread_name);
        let metadata = ThreadMetadataPatch {
            title,
            preview: first_user_message.clone(),
            model_provider: Some(model_provider),
            created_at: Some(now),
            updated_at: Some(now),
            source: Some(source.clone()),
            thread_source: Some(None),
            agent_nickname: Some(source.get_nickname()),
            agent_role: Some(source.get_agent_role()),
            agent_path: Some(source.get_agent_path().map(Into::into)),
            cwd: Some(cwd),
            cli_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            first_user_message,
            memory_mode: Some(memory_mode),
            ..Default::default()
        };

        self.thread_store
            .create_thread(create_params)
            .await
            .map_err(|err| format!("failed to import session: {err}"))?;
        if !rollout_items.is_empty()
            && let Err(err) = self
                .thread_store
                .append_items(AppendThreadItemsParams {
                    thread_id,
                    items: rollout_items,
                })
                .await
        {
            let _ = self.thread_store.discard_thread(thread_id).await;
            return Err(format!("failed to import session: {err}"));
        }

        self.thread_store
            .update_thread_metadata(UpdateThreadMetadataParams {
                thread_id,
                patch: metadata,
                include_archived: false,
            })
            .await
            .map_err(|err| format!("failed to update imported session: {err}"))?;
        self.thread_store
            .persist_thread(thread_id)
            .await
            .map_err(|err| format!("failed to persist imported session: {err}"))?;
        self.thread_store
            .shutdown_thread(thread_id)
            .await
            .map_err(|err| format!("failed to shutdown imported session: {err}"))?;
        Ok(thread_id)
    }
}

struct SessionImportFailure {
    source_path: PathBuf,
    message: String,
    stage: &'static str,
}
