use std::collections::HashSet;
use std::path::Path;

use crate::workflow::artifact_content::{markdown_file_has_content, specs_dir_has_content};
use crate::workflow::change_class::{effective_artifacts, load_change_class};
use crate::workflow::paths::{artifact_exists, change_dir, parse_tasks_progress};
use crate::workflow::schema::{apply_rules, project_context, project_rules, read_template};
use crate::workflow::types::{
    Artifact, ArtifactState, ArtifactStatus, ChangeStatus, WorkflowSchema,
};

fn artifact_done(change_root: &Path, artifact: &Artifact) -> bool {
    if !artifact_exists(change_root, &artifact.generates) {
        return false;
    }
    if artifact.generates.contains('*') {
        if artifact.generates.starts_with("specs/") {
            return specs_dir_has_content(&change_root.join("specs"));
        }
        return true;
    }
    markdown_file_has_content(&change_root.join(&artifact.generates))
}

pub fn compute_change_status(
    root: &Path,
    schema: &WorkflowSchema,
    change_name: &str,
) -> ChangeStatus {
    let dir = change_dir(root, change_name);
    let workflow_class = load_change_class(root, change_name);
    let schema_artifacts = effective_artifacts(schema, workflow_class);
    let content_ready: HashSet<String> = schema_artifacts
        .iter()
        .filter(|artifact| artifact_done(&dir, artifact))
        .map(|artifact| artifact.id.clone())
        .collect();
    let mut done_set = HashSet::new();
    loop {
        let before = done_set.len();
        for artifact in &schema_artifacts {
            if content_ready.contains(&artifact.id)
                && artifact.requires.iter().all(|id| done_set.contains(id))
            {
                done_set.insert(artifact.id.clone());
            }
        }
        if done_set.len() == before {
            break;
        }
    }

    let artifacts: Vec<ArtifactState> = schema_artifacts
        .iter()
        .map(|artifact| {
            let blocked_by: Vec<String> = artifact
                .requires
                .iter()
                .filter(|id| !done_set.contains(*id))
                .cloned()
                .collect();
            let status = if done_set.contains(&artifact.id) {
                ArtifactStatus::Done
            } else if blocked_by.is_empty() {
                ArtifactStatus::Ready
            } else {
                ArtifactStatus::Blocked
            };

            let template_content = if artifact.template.is_some() && status != ArtifactStatus::Done
            {
                read_template(
                    root,
                    artifact.template.as_deref().unwrap_or_default(),
                    &[("CHANGE_NAME", change_name), ("CAPABILITY", "capability")],
                )
                .ok()
            } else {
                None
            };

            ArtifactState {
                id: artifact.id.clone(),
                description: artifact.description.clone(),
                status,
                generates: artifact.generates.clone(),
                output_path: dir.join(&artifact.generates).display().to_string(),
                blocked_by,
                instruction: artifact.instruction.clone(),
                template: template_content,
            }
        })
        .collect();

    let apply_requires = schema
        .apply
        .as_ref()
        .map(|a| a.requires.clone())
        .unwrap_or_else(|| vec!["tasks".to_string()]);
    let apply_blocked_by: Vec<String> = apply_requires
        .iter()
        .filter(|id| !done_set.contains(*id))
        .cloned()
        .collect();
    let tracks = schema
        .apply
        .as_ref()
        .map(|a| a.tracks.as_str())
        .unwrap_or("tasks.md");
    let tasks_path = dir.join(tracks);
    let tasks_progress = parse_tasks_progress(&tasks_path);

    ChangeStatus {
        name: change_name.to_string(),
        change_dir: dir.display().to_string(),
        workflow_class,
        artifacts,
        apply_ready: apply_blocked_by.is_empty(),
        apply_blocked_by,
        tasks_progress,
    }
}

pub fn next_ready_artifact(status: &ChangeStatus) -> Option<String> {
    status
        .artifacts
        .iter()
        .find(|a| a.status == ArtifactStatus::Ready)
        .map(|a| a.id.clone())
}

pub fn load_instructions_payload(
    root: &Path,
    schema: &WorkflowSchema,
    change_name: &str,
    artifact_id: &str,
) -> Option<serde_json::Value> {
    if artifact_id == "apply" {
        let status = compute_change_status(root, schema, change_name);
        let apply_status = if status.apply_ready {
            "ready"
        } else {
            "blocked"
        };
        let tracks = schema
            .apply
            .as_ref()
            .map(|a| a.tracks.as_str())
            .unwrap_or("tasks.md");
        return Some(serde_json::json!({
            "change": change_name,
            "artifact": "apply",
            "status": apply_status,
            "generates": tracks,
            "resolvedOutputPath": format!("{}/{}", status.change_dir, tracks),
            "instruction": schema.apply.as_ref().and_then(|a| a.instruction.clone()).unwrap_or_default(),
            "context": project_context(root),
            "rules": apply_rules(root),
            "blockedBy": status.apply_blocked_by,
            "tasksProgress": status.tasks_progress,
        }));
    }

    let status = compute_change_status(root, schema, change_name);
    let artifact = status.artifacts.iter().find(|a| a.id == artifact_id)?;
    let status_str = match artifact.status {
        ArtifactStatus::Done => "done",
        ArtifactStatus::Ready => "ready",
        ArtifactStatus::Blocked => "blocked",
    };
    Some(serde_json::json!({
        "change": change_name,
        "artifact": artifact_id,
        "status": status_str,
        "generates": artifact.generates,
        "resolvedOutputPath": artifact.output_path,
        "instruction": artifact.instruction.clone().unwrap_or_default(),
        "template": artifact.template.clone().unwrap_or_default(),
        "context": project_context(root),
        "rules": project_rules(root, artifact_id),
        "blockedBy": artifact.blocked_by,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::change_class::ChangeClass;
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
    fn tasks_file_alone_does_not_make_standard_change_apply_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let change = "add-clamp";
        let change_root = change_dir(root, change);
        std::fs::create_dir_all(change_root.join("specs/math")).unwrap();
        std::fs::write(change_root.join("proposal.md"), "# Why\n\nNeed it.\n").unwrap();
        std::fs::write(
            change_root.join("specs/math/spec.md"),
            "### Requirement: bounds\n\nSHALL clamp.\n",
        )
        .unwrap();
        std::fs::write(change_root.join("tasks.md"), "- [ ] Implement\n").unwrap();

        let schema = sample_schema();
        let status = compute_change_status(root, &schema, change);
        assert!(
            !status.apply_ready,
            "tasks.md without design/verification must not unlock apply"
        );
        assert_eq!(status.apply_blocked_by, vec!["tasks".to_string()]);
        let tasks = status
            .artifacts
            .iter()
            .find(|a| a.id == "tasks")
            .expect("tasks artifact");
        assert_eq!(tasks.status, ArtifactStatus::Blocked);
    }

    #[test]
    fn micro_change_apply_ready_with_specs_and_tasks_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let change = "fix-copy";
        let change_root = change_dir(root, change);
        std::fs::create_dir_all(change_root.join("specs/ui")).unwrap();
        crate::workflow::change_class::write_change_meta(root, change, ChangeClass::Micro).unwrap();
        std::fs::write(change_root.join("proposal.md"), "# Why\n\nFix copy.\n").unwrap();
        std::fs::write(
            change_root.join("specs/ui/spec.md"),
            "### Requirement: label\n\nSHALL read Save.\n",
        )
        .unwrap();
        std::fs::write(change_root.join("tasks.md"), "- [ ] Update label\n").unwrap();

        let status = compute_change_status(root, &sample_schema(), change);
        assert!(status.apply_ready);
        assert_eq!(status.workflow_class, ChangeClass::Micro);
    }
}
