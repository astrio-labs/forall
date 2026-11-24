use super::*;

impl AccountRequestProcessor {
    pub(crate) async fn consume_account_rate_limit_reset_credit(
        &self,
        params: ConsumeAccountRateLimitResetCreditParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        if params.idempotency_key.is_empty() {
            return Err(invalid_request("idempotencyKey must not be empty"));
        }

        let Some(_auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "authentication required for rate limit reset credits",
            ));
        };

        Err(hosted_backend_unavailable())
    }
}
