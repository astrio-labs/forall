use std::path::Path;

use crate::mapping::load::load_project_mapping;
use crate::mapping::schema::EvidenceLevel;
use crate::mapping::schema::claimed_evidence_level;
use crate::mapping::validate::{
    validate_mapping_structure, validate_symbols_against_extract, verified_files_from_requirements,
};
use crate::paths::find_project_root;
use crate::verify::adapters::load_verified_config;
use crate::verify::engine::collect_extracts;

pub fn run_mapping_validate(root_arg: Option<&Path>) -> i32 {
    let cwd = root_arg
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall mapping validate: project not found");
        return 1;
    };

    let mapping = match load_project_mapping(&root) {
        Ok(m) => m,
        Err(err) => {
            eprintln!("{err}");
            return 1;
        }
    };
    // Same scope the check uses: version 1 marks it with the `verified`
    // bool, version 2 with an attached machine-checkable artifact. Filtering
    // on the bool alone left a migrated mapping with nothing to validate.
    let version = mapping.version;
    let requirements: Vec<_> = mapping
        .requirements
        .into_iter()
        .filter(|r| {
            if version >= 2 {
                claimed_evidence_level(r, version) >= EvidenceLevel::Tested
            } else {
                r.verified
            }
        })
        .collect();
    let files = verified_files_from_requirements(&requirements, version);
    let config = load_verified_config(&root);
    let (extracts, _) = collect_extracts(&root, &config, &files);
    let issues = [
        validate_mapping_structure(&root, &requirements),
        validate_symbols_against_extract(&root, &requirements, &extracts),
    ]
    .concat();

    if issues.is_empty() {
        println!(
            "mapping validate: OK ({} verified requirement(s))",
            requirements.len()
        );
        return 0;
    }

    for issue in &issues {
        println!("{}: {}", issue.severity.as_str(), issue.message);
    }
    if issues
        .iter()
        .any(|i| i.severity == crate::mapping::schema::IssueSeverity::Critical)
    {
        1
    } else {
        0
    }
}
