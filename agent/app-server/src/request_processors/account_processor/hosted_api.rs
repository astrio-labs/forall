//! Fetches hosted Forall account data (rate limits, token usage) from the
//! Forall gateway for OAuth-authenticated users.
//!
//! The gateway serves camelCase JSON that mirrors the generated TypeScript
//! schema for the v2 app-server protocol (see `gateway/src/wire.rs`). The one
//! intentional difference is that the gateway reports
//! `RateLimitWindow.usedPercent` as a float while the protocol type uses
//! `i32`, so the wire structs below deserialize the gateway payload and map it
//! explicitly into the protocol types.

use super::*;

use forall_app_server_protocol::CreditsSnapshot;
use forall_app_server_protocol::RateLimitReachedType;
use forall_app_server_protocol::RateLimitResetCreditsSummary;
use forall_app_server_protocol::RateLimitSnapshot;
use forall_app_server_protocol::RateLimitWindow;
use forall_app_server_protocol::SpendControlLimitSnapshot;
use forall_model_provider_info::ASTRIO_BASE_URL_ENV_VAR;
use forall_model_provider_info::ASTRIO_DEFAULT_BASE_URL;
use forall_protocol::account::PlanType;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::time::Duration;

const HOSTED_ACCOUNT_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

const RELOGIN_REQUIRED: &str = "Forall authentication is no longer valid; please log in again.";

/// Root of the hosted Forall account API.
///
/// Derived from the astrio model-provider base URL: the `FORALL_API_BASE_URL`
/// env override (with a trailing `/v1` stripped) when set, otherwise the
/// default `https://api.forall.astrio.app`.
pub(crate) fn forall_api_account_base() -> String {
    let base = std::env::var(ASTRIO_BASE_URL_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| ASTRIO_DEFAULT_BASE_URL.to_string());
    let trimmed = base.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

/// Gateway wire shape of `RateLimitWindow`; `used_percent` is a float on the
/// wire but an `i32` in the protocol type.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitWindowWire {
    used_percent: f64,
    #[serde(default)]
    window_duration_mins: Option<i64>,
    #[serde(default)]
    resets_at: Option<i64>,
}

impl From<RateLimitWindowWire> for RateLimitWindow {
    fn from(value: RateLimitWindowWire) -> Self {
        Self {
            used_percent: value.used_percent.round() as i32,
            window_duration_mins: value.window_duration_mins,
            resets_at: value.resets_at,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RateLimitSnapshotWire {
    limit_id: Option<String>,
    limit_name: Option<String>,
    primary: Option<RateLimitWindowWire>,
    secondary: Option<RateLimitWindowWire>,
    credits: Option<CreditsSnapshot>,
    individual_limit: Option<SpendControlLimitSnapshot>,
    plan_type: Option<PlanType>,
    rate_limit_reached_type: Option<RateLimitReachedType>,
}

impl From<RateLimitSnapshotWire> for RateLimitSnapshot {
    fn from(value: RateLimitSnapshotWire) -> Self {
        Self {
            limit_id: value.limit_id,
            limit_name: value.limit_name,
            primary: value.primary.map(RateLimitWindow::from),
            secondary: value.secondary.map(RateLimitWindow::from),
            credits: value.credits,
            individual_limit: value.individual_limit,
            plan_type: value.plan_type,
            rate_limit_reached_type: value.rate_limit_reached_type,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccountRateLimitsResponseWire {
    rate_limits: RateLimitSnapshotWire,
    #[serde(default)]
    rate_limits_by_limit_id: Option<HashMap<String, RateLimitSnapshotWire>>,
    #[serde(default)]
    rate_limit_reset_credits: Option<RateLimitResetCreditsSummary>,
}

impl From<GetAccountRateLimitsResponseWire> for GetAccountRateLimitsResponse {
    fn from(value: GetAccountRateLimitsResponseWire) -> Self {
        Self {
            rate_limits: value.rate_limits.into(),
            rate_limits_by_limit_id: value.rate_limits_by_limit_id.map(|by_limit_id| {
                by_limit_id
                    .into_iter()
                    .map(|(limit_id, snapshot)| (limit_id, snapshot.into()))
                    .collect()
            }),
            rate_limit_reset_credits: value.rate_limit_reset_credits,
        }
    }
}

impl AccountRequestProcessor {
    pub(super) async fn fetch_hosted_rate_limits(
        &self,
        auth: &ForallAuth,
    ) -> Result<GetAccountRateLimitsResponse, JSONRPCErrorError> {
        let wire: GetAccountRateLimitsResponseWire = self
            .hosted_account_get(auth, "/api/account/rate-limits")
            .await?;
        Ok(wire.into())
    }

    pub(super) async fn fetch_hosted_token_usage(
        &self,
        auth: &ForallAuth,
    ) -> Result<GetAccountTokenUsageResponse, JSONRPCErrorError> {
        // The gateway's usage payload is field-for-field isomorphic to the
        // protocol type, so it deserializes directly.
        self.hosted_account_get(auth, "/api/account/usage").await
    }

    /// GETs a hosted account endpoint with the OAuth bearer token, refreshing
    /// the token and retrying once on 401.
    async fn hosted_account_get<T: DeserializeOwned>(
        &self,
        auth: &ForallAuth,
        path: &str,
    ) -> Result<T, JSONRPCErrorError> {
        let url = format!("{}{path}", forall_api_account_base());
        let client = forall_login::default_client::build_reqwest_client();
        let token = auth.get_token().map_err(|err| {
            internal_error(format!("failed to read the Forall access token: {err}"))
        })?;

        let mut response = send_hosted_account_get(&client, &url, &token).await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.auth_manager.refresh_token().await.map_err(|err| {
                invalid_request(format!("{RELOGIN_REQUIRED} (token refresh failed: {err})"))
            })?;
            let refreshed_token = self
                .auth_manager
                .auth_cached()
                .as_ref()
                .and_then(|refreshed| refreshed.get_token().ok())
                .filter(|token| !token.is_empty())
                .ok_or_else(|| invalid_request(RELOGIN_REQUIRED))?;
            response = send_hosted_account_get(&client, &url, &refreshed_token).await?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Err(invalid_request(RELOGIN_REQUIRED));
            }
        }

        let status = response.status();
        if !status.is_success() {
            return Err(internal_error(format!(
                "the Forall API returned HTTP {status} for {path}"
            )));
        }
        response.json::<T>().await.map_err(|err| {
            internal_error(format!(
                "failed to parse the Forall API response from {path}: {err}"
            ))
        })
    }
}

async fn send_hosted_account_get(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<reqwest::Response, JSONRPCErrorError> {
    client
        .get(url)
        .bearer_auth(token)
        .timeout(HOSTED_ACCOUNT_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|err| internal_error(format!("failed to reach the Forall API: {err}")))
}
