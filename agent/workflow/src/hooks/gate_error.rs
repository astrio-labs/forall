use serde::Serialize;

/// Stable gate codes — see docs/verified-engineering/schemas/gate-error.json
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GateErrorCode {
    NoProjectRoot,
    NoActiveChange,
    ApplyNotReady,
    ShellWriteBlocked,
    EnsureFailed,
    CheckCritical,
    WaiverRequired,
    PathNotAllowed,
    VerifiedDowngradeBlocked,
}

impl GateErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            GateErrorCode::NoProjectRoot => "NO_PROJECT_ROOT",
            GateErrorCode::NoActiveChange => "NO_ACTIVE_CHANGE",
            GateErrorCode::ApplyNotReady => "APPLY_NOT_READY",
            GateErrorCode::ShellWriteBlocked => "SHELL_WRITE_BLOCKED",
            GateErrorCode::EnsureFailed => "ENSURE_FAILED",
            GateErrorCode::CheckCritical => "CHECK_CRITICAL",
            GateErrorCode::WaiverRequired => "WAIVER_REQUIRED",
            GateErrorCode::PathNotAllowed => "PATH_NOT_ALLOWED",
            GateErrorCode::VerifiedDowngradeBlocked => "VERIFIED_DOWNGRADE_BLOCKED",
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateErrorPayload {
    pub decision: &'static str,
    pub code: String,
    pub message: String,
    pub remediation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_change: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_artifact: Option<String>,
    pub reason: String,
}

/// Hook stdout: structured gate error + legacy `reason` string for older consumers.
pub fn serialize_pre_tool_use_block(
    code: GateErrorCode,
    message: &str,
    remediation: &str,
    paths: Option<Vec<String>>,
    active_change: Option<String>,
    blocked_by: Option<Vec<String>>,
    next_artifact: Option<String>,
) -> String {
    let reason = format!("{message} {remediation}");
    let block = GateErrorPayload {
        decision: "block",
        code: code.as_str().to_string(),
        message: message.to_string(),
        remediation: remediation.to_string(),
        paths,
        active_change,
        blocked_by,
        next_artifact,
        reason,
    };
    serde_json::to_string(&block).unwrap_or_else(|_| "{}".to_string())
}

pub fn write_pre_tool_use_block(payload: &str) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(payload.as_bytes());
}
