//! `forall mapping migrate`: the explicit v1 → v2 migration.
//!
//! Version 2 mappings carry intent only — the agent-writable `verified` and
//! `property_tested` booleans are gone, and evidence levels are earned
//! through `forall check` and recorded in the obligation ledger. Stripping
//! those claims is a real semantic change (summaries derived from the
//! booleans will report fewer proved requirements until the next check
//! rebuilds the ledger), which is exactly why it is an explicit command with
//! a printed accounting, never a side effect of archiving or loading.

use std::fs;
use std::path::Path;

use crate::mapping::load::{load_project_mapping, mapping_path};
use crate::mapping::schema::validate_mapping;
use crate::paths::find_project_root;

pub struct MigrateOutcome {
    pub stripped_verified: usize,
    pub stripped_property_tested: usize,
}

pub fn migrate_mapping(root: &Path) -> Result<Option<MigrateOutcome>, String> {
    let mut mapping = load_project_mapping(root).map_err(|e| e.to_string())?;
    if mapping.version >= 2 {
        return Ok(None);
    }

    let mut stripped_verified = 0;
    let mut stripped_property_tested = 0;
    for req in &mut mapping.requirements {
        if req.verified {
            req.verified = false;
            stripped_verified += 1;
        }
        if req.property_tested {
            req.property_tested = false;
            stripped_property_tested += 1;
        }
    }
    mapping.version = 2;
    validate_mapping(&mapping)?;

    fs::write(
        mapping_path(root),
        serde_yaml::to_string(&mapping).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    Ok(Some(MigrateOutcome {
        stripped_verified,
        stripped_property_tested,
    }))
}

pub fn run_mapping_migrate(root_arg: Option<&Path>) -> i32 {
    let cwd = root_arg
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall mapping migrate: project not found");
        return 1;
    };

    match migrate_mapping(&root) {
        Ok(None) => {
            println!("forall mapping migrate: already version 2, nothing to do");
            0
        }
        Ok(Some(outcome)) => {
            println!("forall mapping migrate: mapping.yaml is now version 2");
            println!(
                "  stripped claims: {} verified, {} property_tested",
                outcome.stripped_verified, outcome.stripped_property_tested
            );
            println!(
                "  evidence now lives in the verify ledger — run `forall check` to rebuild it"
            );
            0
        }
        Err(err) => {
            eprintln!("forall mapping migrate: {err}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mapping(root: &Path, content: &str) {
        let path = mapping_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn migrates_v1_claims_away_and_bumps_the_version() {
        let dir = tempfile::tempdir().unwrap();
        write_mapping(
            dir.path(),
            concat!(
                "version: 1\n",
                "requirements:\n",
                "  - id: proved-claim\n",
                "    capability: core\n",
                "    requirement: text\n",
                "    verified: true\n",
                "  - id: pbt-claim\n",
                "    capability: core\n",
                "    requirement: text\n",
                "    property_tested: true\n",
                "  - id: plain\n",
                "    capability: core\n",
                "    requirement: text\n",
            ),
        );

        let outcome = migrate_mapping(dir.path()).unwrap().expect("migrated");
        assert_eq!(outcome.stripped_verified, 1);
        assert_eq!(outcome.stripped_property_tested, 1);

        let migrated = load_project_mapping(dir.path()).unwrap();
        assert_eq!(migrated.version, 2);
        assert!(migrated.requirements.iter().all(|r| !r.verified));
        assert!(migrated.requirements.iter().all(|r| !r.property_tested));

        // Second run is a no-op, not an error.
        assert!(migrate_mapping(dir.path()).unwrap().is_none());
    }
}
