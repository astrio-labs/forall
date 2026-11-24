use std::collections::HashSet;
use std::path::Path;

use serde::Deserialize;

use crate::mapping::schema::{IssueSeverity, Requirement, VerifyIssue};
use crate::workflow::paths::change_dir;

#[derive(Debug, Deserialize)]
struct PropertiesFile {
    #[serde(default)]
    properties: Vec<PropertyEntry>,
    #[serde(default)]
    composition: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PropertyEntry {
    id: String,
    requirement_id: String,
    #[serde(default)]
    #[allow(dead_code)] // reserved for future properties.yaml validation
    kind: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    statement: Option<String>,
    #[serde(default)]
    refines: Option<String>,
    #[serde(default)]
    test: Option<String>,
}

pub fn properties_path(root: &Path, change: &str) -> std::path::PathBuf {
    change_dir(root, change).join("properties.yaml")
}

pub fn validate_change_properties(
    root: &Path,
    change: &str,
    requirements: &[Requirement],
) -> Vec<VerifyIssue> {
    let path = properties_path(root, change);
    if !path.is_file() {
        return Vec::new();
    }

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            return vec![VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "properties".to_string(),
                requirement_id: None,
                message: format!("could not read properties.yaml: {err}"),
                file: Some(relative_path(root, &path)),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            }];
        }
    };

    let file: PropertiesFile = match serde_yaml::from_str(&raw) {
        Ok(file) => file,
        Err(err) => {
            return vec![VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "properties".to_string(),
                requirement_id: None,
                message: format!("invalid properties.yaml: {err}"),
                file: Some(relative_path(root, &path)),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            }];
        }
    };

    let req_ids: HashSet<&str> = requirements.iter().map(|r| r.id.as_str()).collect();
    let prop_ids: HashSet<&str> = file.properties.iter().map(|p| p.id.as_str()).collect();
    let rel = relative_path(root, &path);
    let mut issues = Vec::new();

    for entry in &file.properties {
        if !req_ids.contains(entry.requirement_id.as_str()) {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Critical,
                phase: "properties".to_string(),
                requirement_id: Some(entry.requirement_id.clone()),
                message: format!(
                    "property '{}' references unknown requirement_id '{}'",
                    entry.id, entry.requirement_id
                ),
                file: Some(rel.clone()),
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }

        if let Some(refines) = &entry.refines {
            if refines.trim().is_empty() {
                continue;
            }
            if !prop_ids.contains(refines.as_str()) {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Critical,
                    phase: "properties".to_string(),
                    requirement_id: Some(entry.requirement_id.clone()),
                    message: format!(
                        "property '{}' refines unknown property id '{}'",
                        entry.id, refines
                    ),
                    file: Some(rel.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }

        if let Some(test) = &entry.test {
            let test_path = root.join(test);
            if !test_path.is_file() {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Critical,
                    phase: "properties".to_string(),
                    requirement_id: Some(entry.requirement_id.clone()),
                    message: format!("property '{}' test file missing: {test}", entry.id),
                    file: Some(rel.clone()),
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }
    }

    if file.properties.is_empty() && file.composition.is_none() {
        issues.push(VerifyIssue {
            severity: IssueSeverity::Warning,
            phase: "properties".to_string(),
            requirement_id: None,
            message: "properties.yaml is present but defines no properties".to_string(),
            file: Some(rel),
            counterexample: None,
            pbt: None,
            proof_detail: None,
        });
    }

    issues
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::Requirement;

    #[test]
    fn valid_properties_file_passes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let change = "add-clamp";
        std::fs::create_dir_all(crate::workflow::paths::change_dir(root, change)).unwrap();
        std::fs::write(
            properties_path(root, change),
            r#"properties:
  - id: parent
    requirement_id: clamp-bounds
    kind: unit
  - id: child
    requirement_id: clamp-bounds
    refines: parent
"#,
        )
        .unwrap();

        let reqs = vec![Requirement {
            id: "clamp-bounds".to_string(),
            capability: "math".to_string(),
            requirement: "bounds".to_string(),
            verified: false,
            property_tested: false,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        }];

        let issues = validate_change_properties(root, change, &reqs);
        assert!(!issues.iter().any(|i| i.severity == IssueSeverity::Critical));
    }

    #[test]
    fn validates_bad_refines_and_requirement_links() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let change = "add-clamp";
        std::fs::create_dir_all(crate::workflow::paths::change_dir(root, change)).unwrap();
        std::fs::write(
            properties_path(root, change),
            r#"properties:
  - id: parent-prop
    requirement_id: clamp-bounds
    kind: unit
    test: .forall/scenarios/clamp-bounds.property.ts
  - id: child-prop
    requirement_id: clamp-bounds
    kind: unit
    refines: missing-parent
"#,
        )
        .unwrap();

        let reqs = vec![Requirement {
            id: "clamp-bounds".to_string(),
            capability: "math".to_string(),
            requirement: "bounds".to_string(),
            verified: false,
            property_tested: true,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        }];

        let issues = validate_change_properties(root, change, &reqs);
        assert!(
            issues.iter().any(|i| i.message.contains("missing-parent")),
            "expected refines validation issue"
        );
    }
}
