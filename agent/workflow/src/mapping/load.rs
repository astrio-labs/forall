use std::fs;
use std::path::{Path, PathBuf};

use crate::mapping::schema::{
    EvidenceLevel, Mapping, Requirement, claimed_evidence_level, validate_mapping,
};
use crate::paths::forall_mapping_path;
use crate::workflow::paths::change_dir;

pub fn load_mapping(mapping_path: &Path) -> anyhow::Result<Mapping> {
    let raw = fs::read_to_string(mapping_path)?;
    let mapping: Mapping = serde_yaml::from_str(&raw)?;
    validate_mapping(&mapping).map_err(anyhow::Error::msg)?;
    Ok(mapping)
}

pub fn mapping_path(root: &Path) -> PathBuf {
    forall_mapping_path(root)
}

pub fn load_project_mapping(root: &Path) -> anyhow::Result<Mapping> {
    load_mapping(&mapping_path(root))
}

pub fn requirements_for_check(
    root: &Path,
    change: Option<&str>,
) -> anyhow::Result<Vec<Requirement>> {
    let path = mapping_path(root);
    if !path.is_file() {
        // Uninitialized projects (common for hosted GitHub imports) have no mapping yet.
        // Treat as zero verified requirements so structure-only checks can still produce a report.
        return Ok(Vec::new());
    }
    let mapping = load_mapping(&path)?;
    let Some(change) = change else {
        // Project scope means "requirements under active verification". In a
        // version-1 mapping the proof and property flags mark that intent.
        // Version 2 has no claim flags, so the marker is any
        // machine-checkable artifact attached (scenarios, a property, a
        // contract). Without this, a migrated mapping would check nothing.
        let version = mapping.version;
        return Ok(mapping
            .requirements
            .into_iter()
            .filter(|r| {
                if version >= 2 {
                    claimed_evidence_level(r, version) >= EvidenceLevel::Tested
                } else {
                    r.verified || r.property_tested
                }
            })
            .collect());
    };
    let delta_path = change_dir(root, change).join("mapping.delta.yaml");
    if !delta_path.is_file() {
        return Ok(Vec::new());
    }
    let delta = load_mapping(&delta_path)?;
    Ok(delta.requirements)
}

/// The schema version of the mapping file a check run draws its
/// requirements from: the change delta when a change is named, the project
/// mapping otherwise. Missing files count as version 1 (legacy).
pub fn mapping_version_for_check(root: &Path, change: Option<&str>) -> u32 {
    let path = match change {
        Some(change) => change_dir(root, change).join("mapping.delta.yaml"),
        None => mapping_path(root),
    };
    if !path.is_file() {
        return 1;
    }
    load_mapping(&path).map(|m| m.version).unwrap_or(1)
}
