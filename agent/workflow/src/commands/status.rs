use std::path::Path;

use crate::paths::find_project_root;
use crate::workflow::paths::list_active_changes;
use crate::workflow::schema::load_workflow_schema;
use crate::workflow::state::{compute_change_status, load_instructions_payload};
use crate::workflow::status::build_workflow_status;
use crate::workflow::types::ArtifactStatus;

fn print_change_status_human(root: &Path, change_name: &str) -> i32 {
    let Ok(schema) = load_workflow_schema(root) else {
        eprintln!("forall status: could not load workflow schema");
        return 1;
    };
    let status = compute_change_status(root, &schema, change_name);

    println!("change: {change_name}");
    println!();
    for artifact in &status.artifacts {
        let icon = match artifact.status {
            ArtifactStatus::Done => "✓",
            ArtifactStatus::Ready => "→",
            ArtifactStatus::Blocked => "○",
        };
        let status_str = match artifact.status {
            ArtifactStatus::Done => "done",
            ArtifactStatus::Ready => "ready",
            ArtifactStatus::Blocked => "blocked",
        };
        let blocked = if artifact.status != ArtifactStatus::Done && !artifact.blocked_by.is_empty()
        {
            format!(" (blocked by: {})", artifact.blocked_by.join(", "))
        } else {
            String::new()
        };
        println!("  {icon} {}: {status_str}{blocked}", artifact.id,);
    }

    println!();
    if status.apply_ready {
        println!("apply: ready");
        if let Some(tp) = &status.tasks_progress {
            println!("tasks: {}/{} complete", tp.done, tp.total);
        }
    } else {
        println!("apply: blocked by {}", status.apply_blocked_by.join(", "));
    }

    if let Some(next) = status
        .artifacts
        .iter()
        .find(|a| a.status == ArtifactStatus::Ready)
    {
        println!();
        println!(
            "Next: forall instructions {} --change {change_name}",
            next.id
        );
    }

    0
}

pub fn run_status(root: Option<&Path>, change: Option<&str>, json: bool) -> i32 {
    let start = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    if json {
        let snapshot = build_workflow_status(&start, change);
        println!(
            "{}",
            serde_json::to_string_pretty(&snapshot).unwrap_or_default()
        );
        return 0;
    }

    let Some(root) = find_project_root(&start) else {
        eprintln!("forall status: project not found (run `forall init` or `forall ensure`)");
        return 1;
    };

    let changes: Vec<String> = if let Some(c) = change {
        vec![c.to_string()]
    } else {
        list_active_changes(&root)
    };
    if changes.is_empty() {
        println!("No active changes.");
        return 0;
    }

    for (i, name) in changes.iter().enumerate() {
        if changes.len() > 1 && i > 0 {
            println!("---");
        }
        print_change_status_human(&root, name);
    }
    0
}

pub fn run_instructions(artifact: &str, change: &str, root: Option<&Path>, json: bool) -> i32 {
    let cwd = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall instructions: project not found");
        return 1;
    };

    let Ok(schema) = load_workflow_schema(&root) else {
        eprintln!("forall instructions: could not load schema");
        return 1;
    };
    let Some(payload) = load_instructions_payload(&root, &schema, change, artifact) else {
        eprintln!("forall instructions: unknown artifact '{artifact}'");
        return 1;
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        println!("# Instructions: {artifact} ({change})");
        if let Some(instr) = payload.get("instruction").and_then(|v| v.as_str()) {
            println!("{instr}");
        }
        if let Some(template) = payload.get("template").and_then(|v| v.as_str())
            && !template.is_empty()
        {
            println!("\n--- template ---\n");
            println!("{template}");
        }
    }
    0
}

pub fn run_workflow_status(root: Option<&Path>, change: Option<&str>) -> i32 {
    let start = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let snapshot = build_workflow_status(&start, change);
    println!(
        "{}",
        serde_json::to_string_pretty(&snapshot).unwrap_or_default()
    );
    0
}
