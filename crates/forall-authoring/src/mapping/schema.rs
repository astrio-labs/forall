use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodeRef {
    pub file: String,
    pub symbols: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScenarioRef {
    pub name: String,
    #[serde(default)]
    pub kind: Option<ScenarioKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScenarioKind {
    Formal,
    Test,
    Manual,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PropertyRef {
    pub file: String,
    #[serde(default)]
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Requirement {
    pub id: String,
    pub capability: String,
    pub requirement: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub property_tested: bool,
    #[serde(default)]
    pub property: Option<PropertyRef>,
    #[serde(default)]
    pub code: Option<CodeRef>,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub claimcheck: Option<bool>,
    #[serde(default)]
    pub scenarios: Option<Vec<ScenarioRef>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mapping {
    pub version: u32,
    pub requirements: Vec<Requirement>,
}

/// The evidence ladder: how strongly a requirement's claim is backed, ordered
/// so per-path targets and CI ratchets can compare rungs (`Ord` is the
/// feature, not a convenience). STALE is deliberately not a rung — it is a
/// modifier computed from content drift against the recorded fingerprints,
/// never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceLevel {
    /// E0 — nothing links this requirement to code.
    None,
    /// E1 — the requirement maps to code; no machine evidence exists.
    SpecTracked,
    /// E2 — scenario coverage exists.
    Tested,
    /// E3 — a seeded property run passed.
    PropertyTested,
    /// E4 — a formal contract is attached (machine-checkable, not yet
    /// discharged).
    Contracted,
    /// E5 — every obligation discharged by a prover.
    Proved,
}

impl EvidenceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceLevel::None => "none",
            EvidenceLevel::SpecTracked => "spec_tracked",
            EvidenceLevel::Tested => "tested",
            EvidenceLevel::PropertyTested => "property_tested",
            EvidenceLevel::Contracted => "contracted",
            EvidenceLevel::Proved => "proved",
        }
    }
}

/// The ladder rung this requirement CLAIMS, derived from mapping fields
/// alone. In a version-1 mapping the `verified`/`property_tested` booleans
/// are agent-writable, so this is a statement of intent, not a certificate —
/// the earned level lives in the verify ledger, which only `forall check`
/// writes. The two are kept separate on purpose: a claim can never render as
/// evidence.
pub fn claimed_evidence_level(requirement: &Requirement) -> EvidenceLevel {
    if requirement.verified {
        return EvidenceLevel::Proved;
    }
    if requirement.contract.is_some() {
        return EvidenceLevel::Contracted;
    }
    if requirement.property_tested {
        return EvidenceLevel::PropertyTested;
    }
    if requirement
        .scenarios
        .as_ref()
        .is_some_and(|scenarios| !scenarios.is_empty())
    {
        return EvidenceLevel::Tested;
    }
    if requirement.code.is_some() {
        return EvidenceLevel::SpecTracked;
    }
    EvidenceLevel::None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum IssueSeverity {
    Critical,
    Warning,
    Info,
}

impl IssueSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueSeverity::Critical => "CRITICAL",
            IssueSeverity::Warning => "WARNING",
            IssueSeverity::Info => "INFO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PhaseStatus {
    Pass,
    Fail,
    Skip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PbtRunMeta {
    pub seed: u64,
    pub examples_run: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyIssue {
    pub severity: IssueSeverity,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterexample: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbt: Option<PbtRunMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationSummary {
    pub total_requirements: usize,
    pub proved_requirements: usize,
    pub property_tested_requirements: usize,
    pub spec_tracked_requirements: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub ok: bool,
    pub strict: bool,
    pub root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<String>,
    pub phases: std::collections::BTreeMap<String, PhaseStatus>,
    pub issues: Vec<VerifyIssue>,
    pub verified_files: Vec<String>,
    pub verification_summary: VerificationSummary,
}

pub fn summarize_verification(requirements: &[Requirement]) -> VerificationSummary {
    let proved_requirements = requirements.iter().filter(|r| r.verified).count();
    let property_tested_requirements = requirements.iter().filter(|r| r.property_tested).count();
    let spec_tracked_requirements = requirements
        .iter()
        .filter(|r| !r.verified && !r.property_tested)
        .count();
    VerificationSummary {
        total_requirements: requirements.len(),
        proved_requirements,
        property_tested_requirements,
        spec_tracked_requirements,
    }
}

pub fn validate_mapping(mapping: &Mapping) -> Result<(), String> {
    if mapping.version != 1 && mapping.version != 2 {
        return Err(format!(
            "mapping version must be 1 or 2, got {}",
            mapping.version
        ));
    }
    for req in &mapping.requirements {
        if req.id.is_empty() {
            return Err("requirement id must not be empty".to_string());
        }
        if req.capability.is_empty() {
            return Err("requirement capability must not be empty".to_string());
        }
        if req.requirement.is_empty() {
            return Err("requirement text must not be empty".to_string());
        }
        // Version 2 mappings carry intent only. Evidence levels are earned
        // through `forall check` and recorded in the verify ledger — a
        // hand-written claim in the mapping must be a validation error, or
        // the ledger is just the old writable boolean with more steps.
        if mapping.version >= 2 && (req.verified || req.property_tested) {
            return Err(format!(
                "requirement '{}' claims evidence (verified/property_tested) in a version-2 \
                 mapping; evidence is derived from the verify ledger, not written by hand",
                req.id
            ));
        }
        if req.verified && req.property_tested {
            return Err(format!(
                "requirement '{}' cannot set both verified and property_tested (v1: mutually exclusive)",
                req.id
            ));
        }
        if req.property_tested
            && let Some(prop) = &req.property
            && prop.file.is_empty()
        {
            return Err(format!(
                "requirement '{}' property.file must not be empty",
                req.id
            ));
        }
        if let Some(code) = &req.code {
            if code.file.is_empty() {
                return Err("code.file must not be empty".to_string());
            }
            if code.symbols.is_empty() {
                return Err("code.symbols must not be empty".to_string());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod serialization_tests {
    use super::*;

    #[test]
    fn false_flags_are_not_serialized_so_v2_files_stay_flag_free() {
        // Version-2 mappings drop the claim flags entirely; a serde
        // round-trip (migrate, archive merge) must not reintroduce
        // `verified: false` noise into files the docs call flag-free.
        let mapping = Mapping {
            version: 2,
            requirements: vec![Requirement {
                id: "R".to_string(),
                capability: "cap".to_string(),
                requirement: "text".to_string(),
                verified: false,
                property_tested: false,
                property: None,
                code: None,
                contract: None,
                claimcheck: None,
                scenarios: None,
            }],
        };
        let yaml = serde_yaml::to_string(&mapping).unwrap();
        assert!(!yaml.contains("verified"), "got: {yaml}");
        assert!(!yaml.contains("property_tested"), "got: {yaml}");
    }
}

#[cfg(test)]
mod evidence_ladder_tests {
    use super::*;

    fn requirement() -> Requirement {
        Requirement {
            id: "REQ-1".to_string(),
            capability: "cap".to_string(),
            requirement: "text".to_string(),
            verified: false,
            property_tested: false,
            property: None,
            code: None,
            contract: None,
            claimcheck: None,
            scenarios: None,
        }
    }

    #[test]
    fn ladder_orders_rungs_for_ratchet_comparisons() {
        assert!(EvidenceLevel::None < EvidenceLevel::SpecTracked);
        assert!(EvidenceLevel::SpecTracked < EvidenceLevel::Tested);
        assert!(EvidenceLevel::Tested < EvidenceLevel::PropertyTested);
        assert!(EvidenceLevel::PropertyTested < EvidenceLevel::Contracted);
        assert!(EvidenceLevel::Contracted < EvidenceLevel::Proved);
    }

    #[test]
    fn claimed_level_takes_the_highest_rung_the_mapping_states() {
        let mut req = requirement();
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::None);

        req.code = Some(CodeRef {
            file: "src/a.c".to_string(),
            symbols: vec!["f".to_string()],
        });
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::SpecTracked);

        req.scenarios = Some(vec![ScenarioRef {
            name: "s".to_string(),
            kind: None,
        }]);
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::Tested);

        req.property_tested = true;
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::PropertyTested);

        req.contract = Some("Bounds".to_string());
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::Contracted);

        req.property_tested = false;
        req.verified = true;
        assert_eq!(claimed_evidence_level(&req), EvidenceLevel::Proved);
    }

    #[test]
    fn version_2_mappings_reject_hand_written_evidence_claims() {
        let mut req = requirement();
        req.verified = true;
        let mapping = Mapping {
            version: 2,
            requirements: vec![req],
        };
        let err = validate_mapping(&mapping).unwrap_err();
        assert!(err.contains("derived from the verify ledger"), "got: {err}");

        // The same claim is legal in version 1 (legacy semantics).
        let mut legacy_req = requirement();
        legacy_req.verified = true;
        let legacy = Mapping {
            version: 1,
            requirements: vec![legacy_req],
        };
        assert!(validate_mapping(&legacy).is_ok());

        // A claim-free version-2 mapping validates.
        let clean = Mapping {
            version: 2,
            requirements: vec![requirement()],
        };
        assert!(validate_mapping(&clean).is_ok());

        // Unknown versions still fail closed.
        let future = Mapping {
            version: 3,
            requirements: Vec::new(),
        };
        assert!(validate_mapping(&future).is_err());
    }
}
