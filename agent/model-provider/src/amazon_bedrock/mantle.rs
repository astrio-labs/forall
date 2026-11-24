use forall_login::auth::BedrockApiKeyAuth;
use forall_model_provider_info::ModelProviderAwsAuthInfo;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result;

use super::auth::resolve_region;
use super::aws::BedrockEndpoint;

const BEDROCK_MANTLE_SUPPORTED_REGIONS: [&str; 12] = [
    "us-east-2",
    "us-east-1",
    "us-west-2",
    "ap-southeast-3",
    "ap-south-1",
    "ap-northeast-1",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "eu-south-1",
    "eu-north-1",
    "sa-east-1",
];

pub(super) fn base_url(region: &str) -> Result<String> {
    if BEDROCK_MANTLE_SUPPORTED_REGIONS.contains(&region) {
        Ok(format!("https://bedrock-mantle.{region}.api.aws/openai/v1"))
    } else {
        Err(ForallErr::Fatal(format!(
            "Amazon Bedrock Mantle does not support region `{region}`"
        )))
    }
}

pub(super) async fn runtime_base_url(
    managed_auth: Option<&BedrockApiKeyAuth>,
    aws: &ModelProviderAwsAuthInfo,
) -> Result<String> {
    let region = resolve_region(managed_auth, aws, BedrockEndpoint::Mantle).await?;
    base_url(&region)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn base_url_uses_region_endpoint() {
        assert_eq!(
            base_url("ap-northeast-1").expect("supported region"),
            "https://bedrock-mantle.ap-northeast-1.api.aws/openai/v1"
        );
    }

    #[test]
    fn base_url_rejects_unsupported_region() {
        let err = base_url("us-west-1").expect_err("unsupported region");

        assert_eq!(
            err.to_string(),
            "Fatal error: Amazon Bedrock Mantle does not support region `us-west-1`"
        );
    }

    // `aws_auth_config` coverage now lives in `super::aws`, which owns the
    // per-endpoint service name.
}
