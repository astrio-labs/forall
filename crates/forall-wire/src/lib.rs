//! The Forall hosted-verification wire contract — one source of truth.
//!
//! These types define what the hosted verify service emits and accepts.
//! The service (mcp-host), the client crate (forall-hosted-verify), and any
//! external consumer build against THIS crate; the era of hand-mirrored
//! copies held together by twin fixtures ends here. Two consumers cannot
//! use it and keep fixture-based conformance instead: the Python finalizer
//! lambda (until its Rust port lands) and the desktop's TypeScript parser.
//!
//! Evolution rules, unchanged: response-shaped types are additive-tolerant
//! (`serde(default)`, unknown fields ignored); request-shaped types stay
//! strict and evolve only through a coordinated `contract_version` bump.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InlineFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VerificationSource {
    Inline {
        files: Vec<InlineFile>,
    },
    Github {
        repository: String,
        #[serde(rename = "ref")]
        reference: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subdirectory: Option<String>,
    },
}

impl VerificationSource {
    pub fn github(
        repository: impl Into<String>,
        reference: impl Into<String>,
        subdirectory: Option<String>,
    ) -> Self {
        Self::Github {
            repository: repository.into(),
            reference: reference.into(),
            subdirectory,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VerificationScope {
    Project,
    Change { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum VerificationPhase {
    Structure,
    Mapping,
    Proofs,
    Intent,
    Scenarios,
    PropertyTests,
    ScenarioTests,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitVerificationRequest {
    pub source: VerificationSource,
    pub scope: VerificationScope,
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phases: Vec<VerificationPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbt_seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pbt_examples: Option<u32>,
    /// Verify only these requirements — incremental re-verification of a
    /// changed requirement instead of a whole-project run. Absent means
    /// every requirement. Requires contract version 2 on the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_ids: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubmitVerificationResponse {
    pub contract_version: u32,
    pub job_id: String,
    pub status: VerificationStatus,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub source_revision: Option<String>,
    pub poll_after_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobRequest {
    pub job_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Queued,
    Preparing,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationProgress {
    pub phase: String,
    pub completed: u8,
    pub total: u8,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationResult {
    pub ok: bool,
    pub strict: bool,
    #[serde(default)]
    pub phases: BTreeMap<String, VerificationPhaseStatus>,
    #[serde(default)]
    pub issues: Vec<VerificationIssue>,
    #[serde(default)]
    pub verified_files: Vec<String>,
    pub verification_summary: VerificationSummary,
    /// The obligation ledger: per requirement, claimed vs EARNED evidence
    /// level with the prover identity behind each verdict. Absent on runs
    /// predating ledger emission.
    #[serde(default)]
    pub ledger: Option<VerificationLedger>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationLedger {
    pub version: u32,
    #[serde(default)]
    pub change: Option<String>,
    #[serde(default)]
    pub entries: Vec<VerificationLedgerEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationLedgerEntry {
    pub requirement_id: String,
    #[serde(default)]
    pub code: Option<VerificationCodeRef>,
    pub claimed_level: String,
    pub earned_level: String,
    #[serde(default)]
    pub file_sha256: Option<String>,
    #[serde(default)]
    pub symbol_fingerprints: Vec<VerificationSymbolFingerprint>,
    #[serde(default)]
    pub requirement_sha256: Option<String>,
    #[serde(default)]
    pub obligations: Vec<VerificationObligation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationCodeRef {
    pub file: String,
    pub symbols: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationSymbolFingerprint {
    pub symbol: String,
    pub full: String,
    pub code: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationObligation {
    pub description: String,
    pub file: String,
    #[serde(default)]
    pub symbols: Vec<String>,
    pub status: String,
    pub tool: String,
    #[serde(default)]
    pub tool_version: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationIssue {
    pub severity: VerificationIssueSeverity,
    pub phase: String,
    #[serde(default)]
    pub requirement_id: Option<String>,
    pub message: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub counterexample: Option<Value>,
    #[serde(default)]
    pub proof_detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum VerificationPhaseStatus {
    Pass,
    Fail,
    Skip,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum VerificationIssueSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationSummary {
    pub total_requirements: usize,
    pub proved_requirements: usize,
    pub property_tested_requirements: usize,
    pub spec_tracked_requirements: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusVerificationResponse {
    pub contract_version: u32,
    pub job_id: String,
    pub status: VerificationStatus,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub source_revision: Option<String>,
    #[serde(default)]
    pub progress: Option<VerificationProgress>,
    #[serde(default)]
    pub result: Option<VerificationResult>,
    #[serde(default)]
    pub error: Option<VerificationJobError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationJobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

pub type CancelVerificationResponse = StatusVerificationResponse;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExplanationAudience {
    Concise,
    Detailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExplainVerificationRequest {
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issue_indexes: Vec<usize>,
    pub audience: ExplanationAudience,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExplainVerificationResponse {
    pub contract_version: u32,
    pub job_id: String,
    pub summary: String,
    pub actions: Vec<String>,
}
