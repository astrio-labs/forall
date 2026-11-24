use std::fs;
use std::path::{Path, PathBuf};

/// Project workflow + agent integration directory.
pub const FORALL_DIR: &str = ".forall";

/// Legacy workflow directory from before the `.forall/` rename.
pub const LEGACY_FORALL_DIR: &str = "forall";

/// Legacy agent integration subdirectory (pre–flat layout).
pub const LEGACY_AGENT_SUBDIR: &str = "agent";

/// Option B: change planning artifacts.
pub const WORKFLOW_SUBDIR: &str = "workflow";

/// Option B: verification state (mapping, adapters, check cache).
pub const VERIFY_SUBDIR: &str = "verify";

/// Canonical project-root marker (written by `forall init`).
pub const PROJECT_MARKER_REL: &str = ".forall/verify/mapping.yaml";

/// Legacy project-root marker (pre–Option B layout).
pub const LEGACY_PROJECT_MARKER_REL: &str = ".forall/verified/mapping.yaml";

pub fn forall_dir(root: &Path) -> PathBuf {
    root.join(FORALL_DIR)
}

pub fn legacy_agent_dir(root: &Path) -> PathBuf {
    forall_dir(root).join(LEGACY_AGENT_SUBDIR)
}

pub fn workflow_dir(root: &Path) -> PathBuf {
    forall_dir(root).join(WORKFLOW_SUBDIR)
}

pub fn verify_dir(root: &Path) -> PathBuf {
    forall_dir(root).join(VERIFY_SUBDIR)
}

pub fn verify_cache_dir(root: &Path) -> PathBuf {
    verify_dir(root).join("cache")
}

pub fn verify_reports_dir(root: &Path) -> PathBuf {
    verify_cache_dir(root).join("reports")
}

pub fn verify_intent_dir(root: &Path) -> PathBuf {
    verify_cache_dir(root).join("intent")
}

pub fn verify_waivers_dir(root: &Path) -> PathBuf {
    verify_cache_dir(root).join("waivers")
}

pub fn specs_dir(root: &Path) -> PathBuf {
    forall_dir(root).join("specs")
}

pub fn change_specs_dir(root: &Path, change: &str) -> PathBuf {
    workflow_changes_dir(root).join(change).join("specs")
}

pub fn scenarios_dir(root: &Path) -> PathBuf {
    forall_dir(root).join("scenarios")
}

pub fn agents_md_path(root: &Path) -> PathBuf {
    forall_dir(root).join("AGENTS.md")
}

pub fn hooks_json_path(root: &Path) -> PathBuf {
    forall_dir(root).join("hooks.json")
}

pub fn agent_markers_path(root: &Path) -> PathBuf {
    forall_dir(root).join("markers.toml")
}

pub fn workflow_config_path(root: &Path) -> PathBuf {
    workflow_dir(root).join("config.yaml")
}

pub fn workflow_active_path(root: &Path) -> PathBuf {
    workflow_dir(root).join("active")
}

pub fn workflow_changes_dir(root: &Path) -> PathBuf {
    workflow_dir(root).join("changes")
}

pub fn workflow_archive_dir(root: &Path) -> PathBuf {
    workflow_dir(root).join("archive")
}

pub fn verify_mapping_path(root: &Path) -> PathBuf {
    verify_dir(root).join("mapping.yaml")
}

pub fn verify_adapters_path(root: &Path) -> PathBuf {
    verify_dir(root).join("adapters.yaml")
}

fn first_existing_file(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_file()).cloned()
}

fn first_existing_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|p| p.is_dir()).cloned()
}

/// Resolve mapping.yaml for reads (canonical, then legacy layouts).
pub fn forall_mapping_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        verify_mapping_path(root),
        forall_dir(root).join("verified").join("mapping.yaml"),
        root.join(LEGACY_FORALL_DIR)
            .join("verified")
            .join("mapping.yaml"),
    ])
    .unwrap_or_else(|| verify_mapping_path(root))
}

/// Resolve adapters config for reads.
pub fn verify_adapters_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        verify_adapters_path(root),
        verify_dir(root).join("config.yaml"),
        forall_dir(root).join("verified").join("config.yaml"),
    ])
    .unwrap_or_else(|| verify_adapters_path(root))
}

pub fn verify_reports_read_dir(root: &Path) -> PathBuf {
    first_existing_dir(&[
        verify_reports_dir(root),
        verify_dir(root).join("reports"),
        forall_dir(root).join("verified").join("reports"),
    ])
    .unwrap_or_else(|| verify_reports_dir(root))
}

pub fn verify_intent_read_dir(root: &Path) -> PathBuf {
    first_existing_dir(&[
        verify_intent_dir(root),
        verify_dir(root).join("claimcheck"),
        forall_dir(root).join("verified").join("claimcheck"),
    ])
    .unwrap_or_else(|| verify_intent_dir(root))
}

pub fn verify_waivers_read_dir(root: &Path) -> PathBuf {
    first_existing_dir(&[
        verify_waivers_dir(root),
        forall_dir(root).join("session").join("waivers"),
    ])
    .unwrap_or_else(|| verify_waivers_dir(root))
}

pub fn agents_md_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        agents_md_path(root),
        legacy_agent_dir(root).join("AGENTS.md"),
        root.join(LEGACY_FORALL_DIR).join("AGENTS.md"),
    ])
    .unwrap_or_else(|| agents_md_path(root))
}

pub fn hooks_json_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        hooks_json_path(root),
        legacy_agent_dir(root).join("hooks.json"),
    ])
    .unwrap_or_else(|| hooks_json_path(root))
}

pub fn agent_markers_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        agent_markers_path(root),
        legacy_agent_dir(root).join("markers.toml"),
        legacy_agent_dir(root).join("config.toml"),
        forall_dir(root).join("config.toml"),
        forall_dir(root).join(".codex").join("config.toml"),
    ])
    .unwrap_or_else(|| agent_markers_path(root))
}

pub fn workflow_config_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        workflow_config_path(root),
        forall_dir(root).join("config.yaml"),
        root.join(LEGACY_FORALL_DIR).join("config.yaml"),
    ])
    .unwrap_or_else(|| workflow_config_path(root))
}

pub fn workflow_active_read_path(root: &Path) -> PathBuf {
    first_existing_file(&[
        workflow_active_path(root),
        forall_dir(root).join(".active-change"),
        root.join(LEGACY_FORALL_DIR).join(".active-change"),
    ])
    .unwrap_or_else(|| workflow_active_path(root))
}

pub fn workflow_changes_read_dir(root: &Path) -> PathBuf {
    first_existing_dir(&[
        workflow_changes_dir(root),
        forall_dir(root).join("changes"),
        root.join(LEGACY_FORALL_DIR).join("changes"),
    ])
    .unwrap_or_else(|| workflow_changes_dir(root))
}

pub fn workflow_archive_read_dir(root: &Path) -> PathBuf {
    first_existing_dir(&[
        workflow_archive_dir(root),
        workflow_changes_read_dir(root).join("archive"),
        forall_dir(root).join("changes").join("archive"),
    ])
    .unwrap_or_else(|| workflow_archive_dir(root))
}

pub fn project_marker_candidates() -> [&'static str; 2] {
    [PROJECT_MARKER_REL, LEGACY_PROJECT_MARKER_REL]
}

pub fn find_project_root(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir
        .canonicalize()
        .unwrap_or_else(|_| start_dir.to_path_buf());
    loop {
        for marker in project_marker_candidates() {
            if dir.join(marker).is_file() {
                return Some(dir);
            }
        }
        if legacy_forall_mapping_path(&dir).is_file() {
            return Some(dir);
        }
        let parent = dir.parent()?;
        if parent == dir {
            return None;
        }
        dir = parent.to_path_buf();
    }
}

pub fn legacy_forall_mapping_path(root: &Path) -> PathBuf {
    root.join(LEGACY_FORALL_DIR)
        .join("verified")
        .join("mapping.yaml")
}

pub fn resolve_from_root(root: &Path, parts: &[&str]) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in parts {
        p.push(part);
    }
    p
}

pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)
}

pub fn write_if_missing(file_path: &Path, content: &str) -> std::io::Result<()> {
    if !file_path.exists() {
        if let Some(parent) = file_path.parent() {
            ensure_dir(parent)?;
        }
        fs::write(file_path, content)?;
    }
    Ok(())
}

/// Create Option B directory skeleton (idempotent).
pub fn ensure_forall_layout(root: &Path) -> std::io::Result<()> {
    ensure_dir(&forall_dir(root))?;
    ensure_dir(&workflow_dir(root))?;
    ensure_dir(&workflow_changes_dir(root))?;
    ensure_dir(&workflow_archive_dir(root))?;
    ensure_dir(&verify_dir(root))?;
    ensure_dir(&verify_reports_dir(root))?;
    ensure_dir(&verify_intent_dir(root))?;
    ensure_dir(&verify_waivers_dir(root))?;
    Ok(())
}

/// Move legacy `.forall/` layout to Option B when old markers exist.
pub fn migrate_legacy_layout(root: &Path) -> std::io::Result<Vec<String>> {
    let dot = forall_dir(root);
    if !dot.is_dir() {
        return Ok(Vec::new());
    }

    let mut moves: Vec<String> = Vec::new();

    fn mv(from: &Path, to: &Path, log: &mut Vec<String>) -> std::io::Result<()> {
        if !from.exists() || to.exists() {
            return Ok(());
        }
        if let Some(parent) = to.parent() {
            ensure_dir(parent)?;
        }
        fs::rename(from, to)?;
        log.push(format!("{} → {}", from.display(), to.display()));
        Ok(())
    }

    ensure_forall_layout(root)?;

    let legacy_agent = legacy_agent_dir(root);
    if legacy_agent.is_dir() {
        mv(
            &legacy_agent.join("hooks.json"),
            &hooks_json_path(root),
            &mut moves,
        )?;
        mv(
            &legacy_agent.join("markers.toml"),
            &agent_markers_path(root),
            &mut moves,
        )?;
        mv(
            &legacy_agent.join("AGENTS.md"),
            &agents_md_path(root),
            &mut moves,
        )?;
        if legacy_agent
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(&legacy_agent);
        }
    }

    mv(
        &dot.join("config.toml"),
        &agent_markers_path(root),
        &mut moves,
    )?;
    mv(
        &dot.join(".active-change"),
        &workflow_active_path(root),
        &mut moves,
    )?;
    mv(
        &dot.join("config.yaml"),
        &workflow_config_path(root),
        &mut moves,
    )?;

    let legacy_changes = dot.join("changes");
    if legacy_changes.is_dir() {
        for entry in fs::read_dir(&legacy_changes)?.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "archive" {
                for archived in fs::read_dir(entry.path())?.flatten() {
                    mv(
                        &archived.path(),
                        &workflow_archive_dir(root).join(archived.file_name()),
                        &mut moves,
                    )?;
                }
                let _ = fs::remove_dir(entry.path());
            } else if entry.file_type()?.is_dir() {
                mv(
                    &entry.path(),
                    &workflow_changes_dir(root).join(name),
                    &mut moves,
                )?;
            }
        }
        let _ = fs::remove_dir(&legacy_changes);
    }

    mv(
        &dot.join("verified").join("mapping.yaml"),
        &verify_mapping_path(root),
        &mut moves,
    )?;

    let legacy_verified_config = dot.join("verified").join("config.yaml");
    if legacy_verified_config.is_file() {
        fs::remove_file(&legacy_verified_config)?;
        moves.push(format!(
            "removed {} (adapters use built-in defaults)",
            legacy_verified_config.display()
        ));
    }

    let legacy_adapters = verify_adapters_path(root);
    if legacy_adapters.is_file() {
        fs::remove_file(&legacy_adapters)?;
        moves.push(format!(
            "removed {} (adapters use built-in defaults)",
            legacy_adapters.display()
        ));
    }

    let legacy_reports = dot.join("verified").join("reports");
    if legacy_reports.is_dir() {
        for entry in fs::read_dir(&legacy_reports)?.flatten() {
            mv(
                &entry.path(),
                &verify_reports_dir(root).join(entry.file_name()),
                &mut moves,
            )?;
        }
        let _ = fs::remove_dir(&legacy_reports);
    }

    let legacy_claimcheck = dot.join("verified").join("claimcheck");
    if legacy_claimcheck.is_dir() {
        for entry in fs::read_dir(&legacy_claimcheck)?.flatten() {
            mv(
                &entry.path(),
                &verify_intent_dir(root).join(entry.file_name()),
                &mut moves,
            )?;
        }
        let _ = fs::remove_dir(&legacy_claimcheck);
    }

    let legacy_waivers = dot.join("session").join("waivers");
    if legacy_waivers.is_dir() {
        for entry in fs::read_dir(&legacy_waivers)?.flatten() {
            mv(
                &entry.path(),
                &verify_waivers_dir(root).join(entry.file_name()),
                &mut moves,
            )?;
        }
        let _ = fs::remove_dir_all(dot.join("session"));
    }

    let legacy_verified = dot.join("verified");
    if legacy_verified.is_dir() {
        let _ = fs::remove_dir(legacy_verified);
    }

    let legacy_hooks = dot.join("hooks");
    if legacy_hooks.is_dir() {
        let _ = fs::remove_dir_all(legacy_hooks);
    }

    let legacy_schemas = dot.join("schemas");
    if legacy_schemas.is_dir() {
        let _ = fs::remove_dir_all(legacy_schemas);
    }

    Ok(moves)
}

/// Remove duplicate legacy trees and machine-local config from `.forall/`.
pub fn prune_stale_forall_artifacts(root: &Path) -> std::io::Result<Vec<String>> {
    let dot = forall_dir(root);
    if !dot.is_dir() {
        return Ok(Vec::new());
    }

    let mut removed: Vec<String> = Vec::new();

    fn rm_file(path: &Path, log: &mut Vec<String>) -> std::io::Result<()> {
        if path.is_file() {
            fs::remove_file(path)?;
            log.push(format!("removed {}", path.display()));
        }
        Ok(())
    }

    fn rm_dir(path: &Path, log: &mut Vec<String>) -> std::io::Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
            log.push(format!("removed {}", path.display()));
        }
        Ok(())
    }

    rm_file(&verify_adapters_path(root), &mut removed)?;
    rm_file(&dot.join("verified").join("config.yaml"), &mut removed)?;
    rm_file(&hooks_json_path(root), &mut removed)?;

    let workflow_changes = workflow_changes_dir(root);
    let legacy_changes = dot.join("changes");
    if legacy_changes.is_dir() {
        if workflow_changes.is_dir() {
            for entry in fs::read_dir(&legacy_changes)?.flatten() {
                let name = entry.file_name();
                if workflow_changes.join(&name).is_dir() {
                    rm_dir(&entry.path(), &mut removed)?;
                }
            }
            if legacy_changes
                .read_dir()
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false)
            {
                rm_dir(&legacy_changes, &mut removed)?;
            }
        } else {
            rm_dir(&legacy_changes, &mut removed)?;
        }
    }

    rm_dir(&dot.join("hooks"), &mut removed)?;
    rm_dir(&dot.join("schemas"), &mut removed)?;
    rm_dir(&legacy_agent_dir(root), &mut removed)?;

    let legacy_verified = dot.join("verified");
    if legacy_verified.is_dir() && verify_dir(root).is_dir() {
        rm_dir(&legacy_verified, &mut removed)?;
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_project_root_prefers_option_b_marker() {
        let dir = tempfile::tempdir().unwrap();
        ensure_forall_layout(dir.path()).unwrap();
        fs::write(
            verify_mapping_path(dir.path()),
            "version: 1\nrequirements: []\n",
        )
        .unwrap();
        assert_eq!(
            find_project_root(dir.path()).map(|p| p.canonicalize().ok()),
            Some(dir.path().canonicalize().ok())
        );
    }

    #[test]
    fn migrate_legacy_layout_moves_core_paths() {
        let dir = tempfile::tempdir().unwrap();
        let dot = forall_dir(dir.path());
        fs::create_dir_all(dot.join("verified")).unwrap();
        fs::create_dir_all(dot.join("changes").join("my-change")).unwrap();
        fs::write(dot.join("verified/mapping.yaml"), "version: 1\n").unwrap();
        fs::write(dot.join(".active-change"), "my-change\n").unwrap();
        fs::write(dot.join("AGENTS.md"), "# rules\n").unwrap();

        let moves = migrate_legacy_layout(dir.path()).unwrap();
        assert!(!moves.is_empty());
        assert!(workflow_active_path(dir.path()).is_file());
        assert!(agents_md_path(dir.path()).is_file());
        assert!(verify_mapping_path(dir.path()).is_file());
        assert!(workflow_changes_dir(dir.path()).join("my-change").is_dir());
    }

    #[test]
    fn migrate_legacy_layout_flattens_agent_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let dot = forall_dir(dir.path());
        let agent = legacy_agent_dir(dir.path());
        fs::create_dir_all(agent.clone()).unwrap();
        fs::create_dir_all(dot.join("verified")).unwrap();
        fs::write(agent.join("hooks.json"), "{}\n").unwrap();
        fs::write(agent.join("markers.toml"), "project_root_markers = []\n").unwrap();
        fs::write(dot.join("verified/mapping.yaml"), "version: 1\n").unwrap();

        let moves = migrate_legacy_layout(dir.path()).unwrap();
        assert!(moves.iter().any(|m| m.contains("hooks.json")));
        assert!(hooks_json_path(dir.path()).is_file());
        assert!(agent_markers_path(dir.path()).is_file());
        assert!(!agent.exists());
    }
}
