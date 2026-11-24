use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::workflow::paths::change_dir;
use crate::workflow::types::{Artifact, WorkflowSchema};

const CHANGE_META_FILE: &str = "change.yaml";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    Standard,
    Micro,
}

impl ChangeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Micro => "micro",
        }
    }
}

impl std::str::FromStr for ChangeClass {
    type Err = std::convert::Infallible;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(match raw.trim().to_lowercase().as_str() {
            "micro" => Self::Micro,
            _ => Self::Standard,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChangeMeta {
    #[serde(default)]
    class: Option<String>,
}

pub fn load_change_class(root: &Path, change_name: &str) -> ChangeClass {
    let path = change_dir(root, change_name).join(CHANGE_META_FILE);
    if !path.is_file() {
        return ChangeClass::Standard;
    }
    let Ok(raw) = fs::read_to_string(&path) else {
        return ChangeClass::Standard;
    };
    let Ok(meta) = serde_yaml::from_str::<ChangeMeta>(&raw) else {
        return ChangeClass::Standard;
    };
    meta.class
        .as_deref()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(ChangeClass::Standard)
}

pub fn write_change_meta(
    root: &Path,
    change_name: &str,
    class: ChangeClass,
) -> std::io::Result<()> {
    let content = format!("class: {}\n", class.as_str());
    fs::write(
        change_dir(root, change_name).join(CHANGE_META_FILE),
        content,
    )
}

/// Artifacts and dependencies for the active change class.
pub fn effective_artifacts(schema: &WorkflowSchema, class: ChangeClass) -> Vec<Artifact> {
    match class {
        ChangeClass::Standard => schema.artifacts.clone(),
        ChangeClass::Micro => schema
            .artifacts
            .iter()
            .filter(|a| a.id != "design" && a.id != "verification")
            .map(|artifact| {
                if artifact.id == "tasks" {
                    Artifact {
                        requires: vec!["specs".to_string()],
                        ..artifact.clone()
                    }
                } else {
                    artifact.clone()
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::types::{ApplyConfig, WorkflowSchema};

    fn sample_schema() -> WorkflowSchema {
        WorkflowSchema {
            name: "forall".to_string(),
            version: 1,
            description: None,
            artifacts: vec![
                Artifact {
                    id: "proposal".to_string(),
                    generates: "proposal.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec![],
                },
                Artifact {
                    id: "specs".to_string(),
                    generates: "specs/**/*.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec!["proposal".to_string()],
                },
                Artifact {
                    id: "design".to_string(),
                    generates: "design.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec!["proposal".to_string()],
                },
                Artifact {
                    id: "verification".to_string(),
                    generates: "verification.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec!["specs".to_string(), "design".to_string()],
                },
                Artifact {
                    id: "tasks".to_string(),
                    generates: "tasks.md".to_string(),
                    description: None,
                    template: None,
                    instruction: None,
                    requires: vec![
                        "specs".to_string(),
                        "design".to_string(),
                        "verification".to_string(),
                    ],
                },
            ],
            apply: Some(ApplyConfig {
                requires: vec!["tasks".to_string()],
                tracks: "tasks.md".to_string(),
                instruction: None,
            }),
        }
    }

    #[test]
    fn micro_class_skips_design_and_verification() {
        let artifacts = effective_artifacts(&sample_schema(), ChangeClass::Micro);
        let ids: Vec<_> = artifacts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["proposal", "specs", "tasks"]);
        let tasks = artifacts.iter().find(|a| a.id == "tasks").unwrap();
        assert_eq!(tasks.requires, vec!["specs"]);
    }
}
