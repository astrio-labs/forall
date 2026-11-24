use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::mapping::load::mapping_version_for_check;
use crate::mapping::load::requirements_for_check;
use crate::mapping::schema::Requirement;
use crate::verify::check_run::{CheckPhase, PhasePlan};
use crate::verify::property_tests::property_test_path;

fn normalize_rel(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

pub fn normalize_changed_paths(root: &Path, paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .filter_map(|p| normalize_rel(root, p))
        .collect()
}

fn is_ignored_rel(rel: &str) -> bool {
    rel.contains("node_modules")
        || rel.contains("/.git/")
        || rel.starts_with(".git/")
        || rel.contains("/target/")
        || rel.contains("agent/target/")
}

fn requirements_for_rel_paths(
    root: &Path,
    requirements: &[Requirement],
    rel_paths: &[String],
    version: u32,
) -> HashSet<String> {
    let mut ids = HashSet::new();
    for rel in rel_paths {
        if rel.ends_with("mapping.delta.yaml")
            || rel == ".forall/verify/mapping.yaml"
            || rel == ".forall/verified/mapping.yaml"
        {
            return requirements.iter().map(|r| r.id.clone()).collect();
        }
        if rel.starts_with(".forall/scenarios/")
            && (rel.ends_with(".property.ts") || rel.ends_with(".property.py"))
        {
            for req in requirements {
                if let Some(prop_path) = property_test_path(root, req, version)
                    && normalize_rel(root, &prop_path).as_deref() == Some(rel.as_str())
                {
                    ids.insert(req.id.clone());
                }
            }
            continue;
        }
        if rel.starts_with("src/") && (rel.ends_with(".ts") || rel.ends_with(".tsx")) {
            for req in requirements {
                if req
                    .code
                    .as_ref()
                    .map(|c| c.file.replace('\\', "/"))
                    .as_deref()
                    == Some(rel.as_str())
                {
                    ids.insert(req.id.clone());
                }
            }
        }
        if rel.starts_with("src/") && rel.ends_with(".rs") {
            for req in requirements {
                if req
                    .code
                    .as_ref()
                    .map(|c| c.file.replace('\\', "/"))
                    .as_deref()
                    == Some(rel.as_str())
                {
                    ids.insert(req.id.clone());
                }
            }
        }
    }
    ids
}

/// Returns `(phase plan, run_sync)` for a batch of changed paths.
pub fn plan_for_changed_paths(
    root: &Path,
    change: Option<&str>,
    changed_paths: &[PathBuf],
) -> Result<(PhasePlan, bool), String> {
    let rel_paths: Vec<String> = normalize_changed_paths(root, changed_paths)
        .into_iter()
        .filter(|rel| !is_ignored_rel(rel))
        .collect();

    if rel_paths.is_empty() {
        return Ok((PhasePlan::full(), false));
    }

    if rel_paths
        .iter()
        .any(|rel| rel == ".forall/verify/mapping.yaml" || rel == ".forall/verified/mapping.yaml")
    {
        return Ok((PhasePlan::full(), false));
    }

    let requirements = requirements_for_check(root, change).map_err(|e| e.to_string())?;
    let version = mapping_version_for_check(root, change);
    let affected = requirements_for_rel_paths(root, &requirements, &rel_paths, version);

    let only_property = rel_paths.iter().all(|rel| {
        rel.starts_with(".forall/scenarios/")
            && (rel.ends_with(".property.ts") || rel.ends_with(".property.py"))
    });
    if only_property && !affected.is_empty() {
        return Ok((
            PhasePlan {
                phases: BTreeSet::from([CheckPhase::PropertyTests]),
                requirement_ids: Some(affected),
            },
            false,
        ));
    }

    let has_mapping_delta = rel_paths
        .iter()
        .any(|rel| rel.ends_with("mapping.delta.yaml"));
    let has_ts_app = rel_paths
        .iter()
        .any(|rel| rel.starts_with("src/") && (rel.ends_with(".ts") || rel.ends_with(".tsx")));
    let has_rs_app = rel_paths
        .iter()
        .any(|rel| rel.starts_with("src/") && rel.ends_with(".rs"));

    if has_mapping_delta && !has_ts_app && !has_rs_app {
        let mut phases = BTreeSet::from([
            CheckPhase::Mapping,
            CheckPhase::Scenarios,
            CheckPhase::PropertyTests,
            CheckPhase::ScenarioTests,
        ]);
        if requirements
            .iter()
            .any(|r| affected.contains(&r.id) && r.verified)
        {
            phases.insert(CheckPhase::Proofs);
            phases.insert(CheckPhase::Intent);
        }
        return Ok((
            PhasePlan {
                phases,
                requirement_ids: if affected.is_empty() {
                    None
                } else {
                    Some(affected)
                },
            },
            false,
        ));
    }

    if has_rs_app {
        let mut phases = BTreeSet::from([
            CheckPhase::Mapping,
            CheckPhase::Intent,
            CheckPhase::PropertyTests,
            CheckPhase::ScenarioTests,
        ]);
        if requirements
            .iter()
            .any(|r| affected.contains(&r.id) && r.verified)
        {
            phases.insert(CheckPhase::Proofs);
        }
        phases.insert(CheckPhase::Scenarios);
        return Ok((
            PhasePlan {
                phases,
                requirement_ids: if affected.is_empty() {
                    None
                } else {
                    Some(affected)
                },
            },
            false,
        ));
    }

    if has_ts_app {
        let mut phases = BTreeSet::from([
            CheckPhase::Mapping,
            CheckPhase::Intent,
            CheckPhase::PropertyTests,
            CheckPhase::ScenarioTests,
        ]);
        if requirements
            .iter()
            .any(|r| affected.contains(&r.id) && r.verified)
        {
            phases.insert(CheckPhase::Proofs);
        }
        phases.insert(CheckPhase::Scenarios);
        return Ok((
            PhasePlan {
                phases,
                requirement_ids: if affected.is_empty() {
                    None
                } else {
                    Some(affected)
                },
            },
            change.is_some(),
        ));
    }

    Ok((PhasePlan::full(), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active_change::write_active_change_file;
    use crate::commands::workflow::run_init;
    use crate::paths::scenarios_dir;
    use crate::workflow::paths::change_dir;
    use std::fs;

    fn write_plan_fixture(root: &Path) {
        run_init(root, false, false, false);
        fs::create_dir_all(change_dir(root, "add-clamp")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(scenarios_dir(root)).unwrap();
        fs::write(
            root.join("src/clamp.ts"),
            "export function clamp(x: number, lo: number, hi: number): number { return x; }\n",
        )
        .unwrap();
        fs::write(
            root.join(".forall/scenarios/clamp-bounds.property.ts"),
            "export default async () => ({ ok: true });\n",
        )
        .unwrap();
        write_active_change_file(root, "add-clamp").unwrap();
        fs::write(
            change_dir(root, "add-clamp").join("mapping.delta.yaml"),
            r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Within range
    property_tested: true
    property:
      file: .forall/scenarios/clamp-bounds.property.ts
    code:
      file: src/clamp.ts
      symbols: [clamp]
"#,
        )
        .unwrap();
    }

    #[test]
    fn property_file_plans_property_tests_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_plan_fixture(root);
        let path = root.join(".forall/scenarios/clamp-bounds.property.ts");
        let (plan, sync) = plan_for_changed_paths(root, Some("add-clamp"), &[path]).unwrap();
        assert!(!sync);
        assert_eq!(plan.phases.len(), 1);
        assert!(plan.phases.contains(&CheckPhase::PropertyTests));
    }

    #[test]
    fn ts_app_file_plans_sync_and_scoped_phases() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_plan_fixture(root);
        let path = root.join("src/clamp.ts");
        let (plan, sync) = plan_for_changed_paths(root, Some("add-clamp"), &[path]).unwrap();
        assert!(sync);
        assert!(plan.phases.contains(&CheckPhase::Mapping));
        assert!(plan.phases.contains(&CheckPhase::PropertyTests));
        assert!(!plan.phases.contains(&CheckPhase::Structure));
    }

    #[test]
    fn mapping_yaml_plans_full_check() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_plan_fixture(root);
        let path = root.join(".forall/verify/mapping.yaml");
        let (plan, sync) = plan_for_changed_paths(root, Some("add-clamp"), &[path]).unwrap();
        assert!(!sync);
        assert!(plan.phases.contains(&CheckPhase::Structure));
        assert!(plan.requirement_ids.is_none());
    }
}
