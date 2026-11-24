use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::responses_metadata::ForallResponsesMetadata;
use crate::responses_metadata::ForallResponsesRequestKind;
use crate::responses_metadata::filter_extra_metadata;
use crate::responses_metadata::subagent_header_value;
use crate::responses_metadata::subagent_metadata_kind;
use crate::sandbox_tags::permission_profile_sandbox_tag;
use forall_protocol::ThreadId;
use forall_protocol::config_types::WindowsSandboxLevel;
use forall_protocol::models::PermissionProfile;
use forall_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use forall_protocol::protocol::SessionSource;
use forall_protocol::protocol::ThreadSource;
use forall_utils_absolute_path::AbsolutePathBuf;

const MODEL_KEY: &str = "model";
const REASONING_EFFORT_KEY: &str = "reasoning_effort";
const USER_INPUT_REQUESTED_DURING_TURN_KEY: &str = "user_input_requested_during_turn";
const WORKSPACE_KIND_KEY: &str = "workspace_kind";

pub(crate) struct McpTurnMetadataContext<'a> {
    pub(crate) model: &'a str,
    pub(crate) reasoning_effort: Option<ReasoningEffortConfig>,
}

#[allow(clippy::too_many_arguments)]
pub async fn detached_memory_responses_metadata(
    session_id: String,
    thread_id: String,
    window_id: String,
    session_source: &SessionSource,
    sandbox: Option<&str>,
) -> ForallResponsesMetadata {
    ForallResponsesMetadata {
        request_kind: Some(ForallResponsesRequestKind::Memory),
        subagent_header: subagent_header_value(session_source),
        sandbox: sandbox.map(ToString::to_string),
        ..ForallResponsesMetadata::new(session_id, thread_id, window_id)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TurnMetadataState {
    session_id: String,
    thread_id: String,
    forked_from_thread_id: Option<ThreadId>,
    parent_thread_id: Option<ThreadId>,
    subagent_header: Option<String>,
    subagent_kind: Option<String>,
    thread_source: Option<ThreadSource>,
    turn_id: String,
    sandbox: Option<String>,
    turn_started_at_unix_ms: Arc<RwLock<Option<i64>>>,
    responsesapi_client_metadata: Arc<RwLock<BTreeMap<String, String>>>,
    user_input_requested_during_turn: Arc<AtomicBool>,
}

impl TurnMetadataState {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        session_id: String,
        thread_id: String,
        forked_from_thread_id: Option<ThreadId>,
        parent_thread_id: Option<ThreadId>,
        session_source: &SessionSource,
        thread_source: Option<ThreadSource>,
        turn_id: String,
        _cwd: AbsolutePathBuf,
        permission_profile: &PermissionProfile,
        windows_sandbox_level: WindowsSandboxLevel,
        enforce_managed_network: bool,
    ) -> Self {
        let sandbox = Some(
            permission_profile_sandbox_tag(
                permission_profile,
                windows_sandbox_level,
                enforce_managed_network,
            )
            .to_string(),
        );
        Self {
            session_id,
            thread_id,
            forked_from_thread_id,
            parent_thread_id,
            subagent_header: subagent_header_value(session_source),
            subagent_kind: subagent_metadata_kind(session_source),
            thread_source,
            turn_id,
            sandbox,
            turn_started_at_unix_ms: Arc::new(RwLock::new(None)),
            responsesapi_client_metadata: Arc::new(RwLock::new(BTreeMap::new())),
            user_input_requested_during_turn: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn current_meta_value_for_mcp_request(
        &self,
        context: McpTurnMetadataContext<'_>,
    ) -> Option<serde_json::Value> {
        let Value::Object(mut metadata) =
            self.responses_metadata_template().turn_metadata_value()?
        else {
            return None;
        };
        metadata.insert(
            MODEL_KEY.to_string(),
            Value::String(context.model.to_string()),
        );
        match context.reasoning_effort {
            Some(reasoning_effort) => {
                metadata.insert(
                    REASONING_EFFORT_KEY.to_string(),
                    Value::String(reasoning_effort.to_string()),
                );
            }
            None => {
                metadata.remove(REASONING_EFFORT_KEY);
            }
        }
        if self
            .user_input_requested_during_turn
            .load(Ordering::Relaxed)
        {
            metadata.insert(
                USER_INPUT_REQUESTED_DURING_TURN_KEY.to_string(),
                Value::Bool(true),
            );
        } else {
            metadata.remove(USER_INPUT_REQUESTED_DURING_TURN_KEY);
        }
        Some(Value::Object(metadata))
    }

    pub(crate) fn to_responses_metadata(
        &self,
        window_id: String,
        request_kind: ForallResponsesRequestKind,
    ) -> ForallResponsesMetadata {
        ForallResponsesMetadata {
            window_id,
            request_kind: Some(request_kind),
            ..self.responses_metadata_template()
        }
    }

    pub(crate) fn mark_user_input_requested_during_turn(&self) {
        self.user_input_requested_during_turn
            .store(true, Ordering::Relaxed);
    }

    pub(crate) fn set_responsesapi_client_metadata(
        &self,
        responsesapi_client_metadata: HashMap<String, String>,
    ) {
        *self
            .responsesapi_client_metadata
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            filter_extra_metadata(responsesapi_client_metadata);
    }

    pub(crate) fn workspace_kind(&self) -> Option<String> {
        self.responsesapi_client_metadata
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(WORKSPACE_KIND_KEY)
            .cloned()
    }

    fn responses_metadata_template(&self) -> ForallResponsesMetadata {
        ForallResponsesMetadata {
            turn_id: Some(self.turn_id.clone()),
            forked_from_thread_id: self.forked_from_thread_id,
            parent_thread_id: self.parent_thread_id,
            subagent_header: self.subagent_header.clone(),
            subagent_kind: self.subagent_kind.clone(),
            thread_source: self.thread_source.clone(),
            sandbox: self.sandbox.clone(),
            turn_started_at_unix_ms: self.current_turn_started_at_unix_ms(),
            extra: self
                .responsesapi_client_metadata
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
            ..ForallResponsesMetadata::new(
                self.session_id.clone(),
                self.thread_id.clone(),
                String::new(),
            )
        }
    }

    fn current_turn_started_at_unix_ms(&self) -> Option<i64> {
        *self
            .turn_started_at_unix_ms
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn set_turn_started_at_unix_ms(&self, turn_started_at_unix_ms: i64) {
        *self
            .turn_started_at_unix_ms
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(turn_started_at_unix_ms);
    }
}

#[cfg(test)]
#[path = "turn_metadata_tests.rs"]
mod tests;
