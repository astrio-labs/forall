use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

pub trait RolloutConfigView {
    fn forall_home(&self) -> &Path;
    fn sqlite_home(&self) -> &Path;
    fn cwd(&self) -> &Path;
    fn model_provider_id(&self) -> &str;
    fn generate_memories(&self) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolloutConfig {
    pub forall_home: PathBuf,
    pub sqlite_home: PathBuf,
    pub cwd: PathBuf,
    pub model_provider_id: String,
    pub generate_memories: bool,
}

pub type Config = RolloutConfig;

impl RolloutConfig {
    pub fn from_view(view: &impl RolloutConfigView) -> Self {
        Self {
            forall_home: view.forall_home().to_path_buf(),
            sqlite_home: view.sqlite_home().to_path_buf(),
            cwd: view.cwd().to_path_buf(),
            model_provider_id: view.model_provider_id().to_string(),
            generate_memories: view.generate_memories(),
        }
    }
}

impl RolloutConfigView for RolloutConfig {
    fn forall_home(&self) -> &Path {
        self.forall_home.as_path()
    }

    fn sqlite_home(&self) -> &Path {
        self.sqlite_home.as_path()
    }

    fn cwd(&self) -> &Path {
        self.cwd.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }

    fn generate_memories(&self) -> bool {
        self.generate_memories
    }
}

impl<T: RolloutConfigView + ?Sized> RolloutConfigView for &T {
    fn forall_home(&self) -> &Path {
        (*self).forall_home()
    }

    fn sqlite_home(&self) -> &Path {
        (*self).sqlite_home()
    }

    fn cwd(&self) -> &Path {
        (*self).cwd()
    }

    fn model_provider_id(&self) -> &str {
        (*self).model_provider_id()
    }

    fn generate_memories(&self) -> bool {
        (*self).generate_memories()
    }
}

impl<T: RolloutConfigView + ?Sized> RolloutConfigView for Arc<T> {
    fn forall_home(&self) -> &Path {
        self.as_ref().forall_home()
    }

    fn sqlite_home(&self) -> &Path {
        self.as_ref().sqlite_home()
    }

    fn cwd(&self) -> &Path {
        self.as_ref().cwd()
    }

    fn model_provider_id(&self) -> &str {
        self.as_ref().model_provider_id()
    }

    fn generate_memories(&self) -> bool {
        self.as_ref().generate_memories()
    }
}
