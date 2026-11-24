use crate::config::Config;
pub use forall_rollout::ARCHIVED_SESSIONS_SUBDIR;
pub use forall_rollout::Cursor;
pub use forall_rollout::INTERACTIVE_SESSION_SOURCES;
pub use forall_rollout::RolloutRecorder;
pub use forall_rollout::RolloutRecorderParams;
pub use forall_rollout::SESSIONS_SUBDIR;
pub use forall_rollout::SessionMeta;
pub use forall_rollout::SortDirection;
pub use forall_rollout::ThreadItem;
pub use forall_rollout::ThreadSortKey;
pub use forall_rollout::ThreadsPage;
pub use forall_rollout::append_thread_name;
pub use forall_rollout::find_archived_thread_path_by_id_str;
#[deprecated(note = "use find_thread_path_by_id_str")]
pub use forall_rollout::find_conversation_path_by_id_str;
pub use forall_rollout::find_thread_meta_by_name_str;
pub use forall_rollout::find_thread_name_by_id;
pub use forall_rollout::find_thread_names_by_ids;
pub use forall_rollout::find_thread_path_by_id_str;
pub use forall_rollout::parse_cursor;
pub use forall_rollout::read_head_for_summary;
pub use forall_rollout::read_session_meta_line;
pub use forall_rollout::rollout_date_parts;

impl forall_rollout::RolloutConfigView for Config {
    fn forall_home(&self) -> &std::path::Path {
        self.forall_home.as_path()
    }

    fn sqlite_home(&self) -> &std::path::Path {
        self.sqlite_home.as_path()
    }

    fn cwd(&self) -> &std::path::Path {
        self.cwd.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }

    fn generate_memories(&self) -> bool {
        self.memories.generate_memories
    }
}

pub(crate) mod list {
    pub use forall_rollout::find_thread_path_by_id_str;
}

#[cfg(test)]
pub(crate) mod recorder {
    pub use forall_rollout::RolloutRecorder;
}

pub(crate) use crate::session_rollout_init_error::map_session_init_error;

pub(crate) mod truncation {
    pub(crate) use crate::thread_rollout_truncation::*;
}
