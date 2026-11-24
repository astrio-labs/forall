//! Rate-limit error classification for app-server turn failures.

use forall_app_server_protocol::ForallErrorInfo as AppServerForallErrorInfo;

#[derive(Debug)]
pub(super) enum RateLimitErrorKind {
    ServerOverloaded,
    UsageLimit,
    Generic,
}

pub(super) fn app_server_rate_limit_error_kind(
    info: &AppServerForallErrorInfo,
) -> Option<RateLimitErrorKind> {
    match info {
        AppServerForallErrorInfo::ServerOverloaded => Some(RateLimitErrorKind::ServerOverloaded),
        AppServerForallErrorInfo::UsageLimitExceeded => Some(RateLimitErrorKind::UsageLimit),
        AppServerForallErrorInfo::ResponseTooManyFailedAttempts {
            http_status_code: Some(429),
        } => Some(RateLimitErrorKind::Generic),
        _ => None,
    }
}

pub(super) fn is_app_server_cyber_policy_error(info: &AppServerForallErrorInfo) -> bool {
    matches!(info, AppServerForallErrorInfo::CyberPolicy)
}
