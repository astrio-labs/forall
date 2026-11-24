//! Shared AWS configuration for the two Bedrock front doors.
//!
//! Forall talks to Bedrock through two unrelated endpoints, and they sign as
//! *different services*:
//!
//! | endpoint                            | SigV4 service    | wire protocol   |
//! |-------------------------------------|------------------|-----------------|
//! | `bedrock-mantle.{region}.api.aws`   | `bedrock-mantle` | Responses (GPT) |
//! | `bedrock-runtime.{region}.amazonaws.com` | `bedrock`   | Messages (Claude) |
//!
//! Signing with the wrong service name yields a signature the service rejects,
//! so the service name is carried by [`BedrockEndpoint`] rather than being a
//! free-floating string at each call site.

use forall_aws_auth::AwsAuthConfig;
use forall_model_provider_info::ModelProviderAwsAuthInfo;

/// Which Bedrock front door a request is bound for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BedrockEndpoint {
    /// The OpenAI-compatible Mantle front door.
    Mantle,
    /// The native `bedrock-runtime` invoke endpoints.
    Runtime,
}

impl BedrockEndpoint {
    /// SigV4 service name this endpoint verifies signatures against.
    pub(super) fn sigv4_service(self) -> &'static str {
        match self {
            Self::Mantle => "bedrock-mantle",
            Self::Runtime => "bedrock",
        }
    }
}

pub(super) fn aws_auth_config(
    aws: &ModelProviderAwsAuthInfo,
    endpoint: BedrockEndpoint,
) -> AwsAuthConfig {
    AwsAuthConfig {
        profile: aws.profile.clone(),
        region: region_from_config(aws),
        service: endpoint.sigv4_service().to_string(),
    }
}

pub(super) fn region_from_config(aws: &ModelProviderAwsAuthInfo) -> Option<String> {
    aws.region
        .as_deref()
        .map(str::trim)
        .filter(|region| !region.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn each_front_door_signs_as_its_own_service() {
        assert_eq!(BedrockEndpoint::Mantle.sigv4_service(), "bedrock-mantle");
        assert_eq!(BedrockEndpoint::Runtime.sigv4_service(), "bedrock");
    }

    #[test]
    fn mantle_auth_config_uses_profile_and_mantle_service() {
        assert_eq!(
            aws_auth_config(
                &ModelProviderAwsAuthInfo {
                    profile: Some("forall-bedrock".to_string()),
                    region: None,
                },
                BedrockEndpoint::Mantle,
            ),
            AwsAuthConfig {
                profile: Some("forall-bedrock".to_string()),
                region: None,
                service: "bedrock-mantle".to_string(),
            }
        );
    }

    #[test]
    fn mantle_auth_config_uses_configured_region() {
        assert_eq!(
            aws_auth_config(
                &ModelProviderAwsAuthInfo {
                    profile: None,
                    region: Some(" us-west-2 ".to_string()),
                },
                BedrockEndpoint::Mantle,
            ),
            AwsAuthConfig {
                profile: None,
                region: Some("us-west-2".to_string()),
                service: "bedrock-mantle".to_string(),
            }
        );
    }

    #[test]
    fn runtime_auth_config_targets_the_bedrock_service() {
        assert_eq!(
            aws_auth_config(
                &ModelProviderAwsAuthInfo {
                    profile: Some("forall-bedrock".to_string()),
                    region: Some(" us-west-2 ".to_string()),
                },
                BedrockEndpoint::Runtime,
            ),
            AwsAuthConfig {
                profile: Some("forall-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
                service: "bedrock".to_string(),
            }
        );
    }

    #[test]
    fn blank_configured_regions_are_treated_as_unset() {
        assert_eq!(
            region_from_config(&ModelProviderAwsAuthInfo {
                profile: None,
                region: Some("   ".to_string()),
            }),
            None
        );
    }
}
