use std::collections::HashMap;
use std::fs;
use std::path::Path;

use regex::Regex;
use std::sync::LazyLock;

use crate::mapping::schema::{IssueSeverity, Requirement, ScenarioKind, VerifyIssue};
use crate::paths::{change_specs_dir, specs_dir};
use crate::specs::requirement_blocks::{extract_requirements_section, normalize_requirement_name};

static SCENARIO_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^####\s+Scenario:\s*(.+)\s*$")
        .unwrap_or_else(|err| panic!("invalid scenario header regex: {err}"))
});

fn resolve_spec_path(
    root: &Path,
    change: Option<&str>,
    capability: &str,
) -> Option<std::path::PathBuf> {
    if let Some(change) = change {
        let change_path = change_specs_dir(root, change)
            .join(capability)
            .join("spec.md");
        if change_path.is_file() {
            return Some(change_path);
        }
    }
    let global_path = specs_dir(root).join(capability).join("spec.md");
    if global_path.is_file() {
        return Some(global_path);
    }
    None
}

fn parse_spec_scenarios(spec_content: &str) -> HashMap<String, Vec<String>> {
    let parts = extract_requirements_section(spec_content);
    let mut result = HashMap::new();
    for block in parts.body_blocks {
        let mut scenarios = Vec::new();
        for line in block.raw.lines() {
            if let Some(name) = SCENARIO_HEADER.captures(line).and_then(|caps| caps.get(1)) {
                scenarios.push(normalize_requirement_name(name.as_str()));
            }
        }
        result.insert(normalize_requirement_name(&block.name), scenarios);
    }
    result
}

fn requirement_name_to_kebab(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

fn spec_names_for_requirement(
    spec_scenarios: &HashMap<String, Vec<String>>,
    req: &Requirement,
) -> Vec<String> {
    let by_title = normalize_requirement_name(&req.requirement);
    let by_id = normalize_requirement_name(&req.id);
    if let Some(names) = spec_scenarios.get(&by_title) {
        return names.clone();
    }
    if let Some(names) = spec_scenarios.get(&by_id) {
        return names.clone();
    }
    for (spec_name, names) in spec_scenarios {
        if requirement_name_to_kebab(spec_name) == by_id {
            return names.clone();
        }
    }
    Vec::new()
}

fn load_capability_scenarios(
    root: &Path,
    change: Option<&str>,
    capability: &str,
) -> Option<HashMap<String, Vec<String>>> {
    let file = resolve_spec_path(root, change, capability)?;
    let content = fs::read_to_string(file).ok()?;
    Some(parse_spec_scenarios(&content))
}

pub fn validate_scenarios(
    root: &Path,
    change: Option<&str>,
    requirements: &[Requirement],
) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();
    let mut cache: HashMap<String, Option<HashMap<String, Vec<String>>>> = HashMap::new();

    for req in requirements {
        if !cache.contains_key(&req.capability) {
            cache.insert(
                req.capability.clone(),
                load_capability_scenarios(root, change, &req.capability),
            );
        }
        let spec_scenarios = cache
            .get(&req.capability)
            .and_then(std::clone::Clone::clone);
        let mapping_scenarios = req.scenarios.clone().unwrap_or_default();

        let Some(spec_scenarios) = spec_scenarios else {
            if req.verified || !mapping_scenarios.is_empty() {
                let hint = change
                    .map(|c| format!(
                        ".forall/workflow/changes/{c}/specs/{}/spec.md or .forall/specs/{}/spec.md",
                        req.capability, req.capability
                    ))
                    .unwrap_or_else(|| {
                        format!(".forall/specs/{}/spec.md", req.capability)
                    });
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "scenarios".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!("spec not found: {hint}"),
                    file: None,
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
            continue;
        };

        let spec_names = spec_names_for_requirement(&spec_scenarios, req);

        if spec_names.is_empty() {
            issues.push(VerifyIssue {
                severity: if req.verified {
                    IssueSeverity::Critical
                } else {
                    IssueSeverity::Warning
                },
                phase: "scenarios".to_string(),
                requirement_id: Some(req.id.clone()),
                message: format!("requirement '{}' has no scenarios in spec", req.requirement),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }

        if mapping_scenarios.is_empty() && req.verified {
            issues.push(VerifyIssue {
                severity: IssueSeverity::Warning,
                phase: "scenarios".to_string(),
                requirement_id: Some(req.id.clone()),
                message: "verified requirement has no scenarios listed in mapping.yaml".to_string(),
                file: None,
                counterexample: None,
                pbt: None,
                proof_detail: None,
            });
        }

        for scenario in &mapping_scenarios {
            let scenario_name = normalize_requirement_name(&scenario.name);
            let in_spec = spec_names
                .iter()
                .any(|s| normalize_requirement_name(s) == scenario_name);
            if !in_spec {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "scenarios".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!(
                        "mapping scenario '{}' not found under '{}' in spec",
                        scenario.name, req.requirement
                    ),
                    file: None,
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }

            if scenario.kind == Some(ScenarioKind::Formal) && req.verified && !in_spec {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Critical,
                    phase: "scenarios".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!(
                        "formal scenario '{}' missing from spec for verified requirement",
                        scenario.name
                    ),
                    file: None,
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }

        for spec_scenario in &spec_names {
            let tracked = mapping_scenarios.iter().any(|s| {
                normalize_requirement_name(&s.name) == normalize_requirement_name(spec_scenario)
            });
            if req.verified && !tracked {
                issues.push(VerifyIssue {
                    severity: IssueSeverity::Warning,
                    phase: "scenarios".to_string(),
                    requirement_id: Some(req.id.clone()),
                    message: format!("spec scenario '{spec_scenario}' not listed in mapping.yaml"),
                    file: None,
                    counterexample: None,
                    pbt: None,
                    proof_detail: None,
                });
            }
        }
    }

    issues
}

pub fn scenarios_phase_status(
    requirements: &[Requirement],
    issues: &[VerifyIssue],
) -> crate::mapping::schema::PhaseStatus {
    use crate::mapping::schema::PhaseStatus;
    if requirements.is_empty() {
        return PhaseStatus::Skip;
    }
    let has_spec_work = requirements
        .iter()
        .any(|r| r.verified || r.scenarios.as_ref().is_some_and(|s| !s.is_empty()));
    if !has_spec_work {
        return PhaseStatus::Skip;
    }
    if issues
        .iter()
        .any(|i| i.phase == "scenarios" && i.severity == IssueSeverity::Critical)
    {
        return PhaseStatus::Fail;
    }
    PhaseStatus::Pass
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::Requirement;

    #[test]
    fn resolves_change_specs_before_global_specs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let change = "my-change";
        let spec_dir = root
            .join(".forall/workflow/changes")
            .join(change)
            .join("specs/game");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(
            spec_dir.join("spec.md"),
            "## Requirements\n\n### Requirement: game\n\n#### Scenario: clamp-bounds\n",
        )
        .unwrap();

        let req = Requirement {
            id: "req-1".to_string(),
            capability: "game".to_string(),
            requirement: "game".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: Some(vec![crate::mapping::schema::ScenarioRef {
                name: "clamp-bounds".to_string(),
                kind: None,
            }]),
        };

        let issues = validate_scenarios(root, Some(change), &[req]);
        assert!(
            issues.is_empty(),
            "expected change spec to satisfy scenario tracking: {issues:?}"
        );
    }

    #[test]
    fn resolves_spec_scenarios_by_requirement_id_when_title_differs() {
        use super::*;
        use crate::mapping::schema::Requirement;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let change = "my-change";
        let spec_dir = root
            .join(".forall/workflow/changes")
            .join(change)
            .join("specs/game");
        std::fs::create_dir_all(&spec_dir).unwrap();
        std::fs::write(
            spec_dir.join("spec.md"),
            "## Requirements\n\n### Requirement: Validate palette colors\n\n#### Scenario: Known palette color\n",
        )
        .unwrap();

        let req = Requirement {
            id: "validate-palette-colors".to_string(),
            capability: "game".to_string(),
            requirement: "validate-palette-colors".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: Some(vec![crate::mapping::schema::ScenarioRef {
                name: "Known palette color".to_string(),
                kind: None,
            }]),
        };

        let issues = validate_scenarios(root, Some(change), &[req]);
        assert!(
            !issues.iter().any(|i| i.severity == IssueSeverity::Critical),
            "expected id fallback to find spec scenarios: {issues:?}"
        );
    }
}
