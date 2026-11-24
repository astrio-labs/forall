//! Endpoint resolution for the native `bedrock-runtime` invoke API.

use forall_login::auth::BedrockApiKeyAuth;
use forall_model_provider_info::ModelProviderAwsAuthInfo;
use forall_protocol::error::ForallErr;
use forall_protocol::error::Result;

use super::auth::resolve_region;
use super::aws::BedrockEndpoint;

/// Regions where `bedrock-runtime` is documented as available.
///
/// Deliberately *not* the Mantle list: Mantle's OpenAI-compatible front door is
/// live in 12 regions, while `bedrock-runtime` is far broader. The check mirrors
/// [`super::mantle::base_url`]'s: an unlisted region is rejected locally with a
/// clear message rather than producing a DNS failure at request time.
///
/// This list is derived from AWS documentation, not probed at runtime, so it
/// will lag new region launches; adding a region here is the fix. Regions in
/// partitions whose Bedrock endpoints do not follow the
/// `bedrock-runtime.{region}.amazonaws.com` pattern (notably `cn-*`, which uses
/// `.amazonaws.com.cn`) are deliberately absent — constructing a URL for them
/// would be wrong, not merely unsupported.
const BEDROCK_RUNTIME_SUPPORTED_REGIONS: [&str; 28] = [
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "ca-central-1",
    "ca-west-1",
    "sa-east-1",
    "mx-central-1",
    "eu-central-1",
    "eu-central-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "ap-east-1",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-northeast-1",
    "ap-northeast-2",
];

/// Base URL for the `bedrock-runtime` endpoint in `region`.
///
/// No path suffix: the endpoint's paths are model-scoped
/// (`model/{modelId}/invoke-with-response-stream`) and carry no API version
/// segment of their own.
pub(super) fn base_url(region: &str) -> Result<String> {
    if BEDROCK_RUNTIME_SUPPORTED_REGIONS.contains(&region) {
        Ok(format!("https://bedrock-runtime.{region}.amazonaws.com"))
    } else {
        Err(ForallErr::Fatal(format!(
            "Amazon Bedrock is not available in region `{region}`"
        )))
    }
}

pub(super) async fn runtime_base_url(
    managed_auth: Option<&BedrockApiKeyAuth>,
    aws: &ModelProviderAwsAuthInfo,
) -> Result<String> {
    let region = resolve_region(managed_auth, aws, BedrockEndpoint::Runtime).await?;
    base_url(&region)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn base_url_uses_the_region_endpoint_without_a_path_suffix() {
        assert_eq!(
            base_url("eu-central-1").expect("supported region"),
            "https://bedrock-runtime.eu-central-1.amazonaws.com"
        );
    }

    #[test]
    fn base_url_covers_regions_mantle_does_not() {
        // The point of a separate region list: these are valid for
        // `bedrock-runtime` but rejected by the Mantle front door.
        for region in ["us-west-1", "ap-northeast-2", "ca-central-1"] {
            assert!(
                base_url(region).is_ok(),
                "{region} should be a valid bedrock-runtime region"
            );
            assert!(
                super::super::mantle::base_url(region).is_err(),
                "{region} is expected to be outside Mantle's region list"
            );
        }
    }

    #[test]
    fn base_url_rejects_regions_outside_the_commercial_partition_pattern() {
        // `cn-north-1` resolves under `.amazonaws.com.cn`, so the constructed
        // URL would be wrong rather than merely unsupported.
        let err = base_url("cn-north-1").expect_err("unsupported partition");

        assert_eq!(
            err.to_string(),
            "Fatal error: Amazon Bedrock is not available in region `cn-north-1`"
        );
    }

    #[test]
    fn base_url_rejects_an_unknown_region() {
        assert!(base_url("moon-base-1").is_err());
    }
}
