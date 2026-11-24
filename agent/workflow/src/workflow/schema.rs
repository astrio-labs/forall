use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::bundled::bundled_text;
use crate::paths::workflow_config_read_path;
use crate::workflow::types::WorkflowSchema;

pub fn load_workflow_schema(_root: &Path) -> anyhow::Result<WorkflowSchema> {
    let raw = bundled_text("schema.yaml")?;
    let schema: WorkflowSchema = serde_yaml::from_str(raw)?;
    Ok(schema)
}

pub fn read_template(
    _root: &Path,
    template_name: &str,
    vars: &[(&str, &str)],
) -> std::io::Result<String> {
    let mut content = bundled_text(Path::new("templates").join(template_name))?.to_string();
    for (key, value) in vars {
        content = content.replace(&format!("{{{{{key}}}}}"), value);
    }
    Ok(content)
}

#[derive(Debug, Default, Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    rules: Option<std::collections::HashMap<String, Vec<String>>>,
}

pub fn load_project_config(root: &Path) -> ProjectConfig {
    let config_path = workflow_config_read_path(root);
    if !config_path.is_file() {
        return ProjectConfig::default();
    }
    fs::read_to_string(config_path)
        .ok()
        .and_then(|raw| serde_yaml::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn project_context(root: &Path) -> String {
    load_project_config(root).context.unwrap_or_default()
}

pub fn project_rules(root: &Path, artifact_id: &str) -> Vec<String> {
    let config = load_project_config(root);
    let rules = config.rules.unwrap_or_default();
    rules
        .get(artifact_id)
        .cloned()
        .or_else(|| rules.get("verification").cloned())
        .unwrap_or_default()
}

pub fn apply_rules(root: &Path) -> Vec<String> {
    let config = load_project_config(root);
    let rules = config.rules.unwrap_or_default();
    rules
        .get("apply")
        .cloned()
        .or_else(|| rules.get("verification").cloned())
        .unwrap_or_default()
}
