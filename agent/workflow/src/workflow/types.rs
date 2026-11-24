use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub generates: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApplyConfig {
    pub requires: Vec<String>,
    pub tracks: String,
    #[serde(default)]
    pub instruction: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkflowSchema {
    pub name: String,
    pub version: u32,
    #[serde(default)]
    pub description: Option<String>,
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub apply: Option<ApplyConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactStatus {
    Done,
    Ready,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct ArtifactState {
    pub id: String,
    pub description: Option<String>,
    pub status: ArtifactStatus,
    pub generates: String,
    pub output_path: String,
    pub blocked_by: Vec<String>,
    pub instruction: Option<String>,
    pub template: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChangeStatus {
    pub name: String,
    pub change_dir: String,
    pub workflow_class: crate::workflow::change_class::ChangeClass,
    pub artifacts: Vec<ArtifactState>,
    pub apply_ready: bool,
    pub apply_blocked_by: Vec<String>,
    pub tasks_progress: Option<TasksProgress>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TasksProgress {
    pub done: usize,
    pub total: usize,
}
