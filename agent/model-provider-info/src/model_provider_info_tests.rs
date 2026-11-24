use super::*;
use forall_utils_absolute_path::AbsolutePathBuf;
use forall_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.openai.azure.com/openai"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.openai.azure.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".to_string(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".to_string(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api_shows_helpful_error() {
    let provider_toml = r#"
name = "OpenAI using Chat Completions"
base_url = "https://api.openai.com/v1"
env_key = "OPENAI_API_KEY"
wire_api = "chat"
        "#;

    let err = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap_err();
    assert!(err.to_string().contains(CHAT_WIRE_API_REMOVED_ERROR));
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.openai.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn test_supports_remote_compaction_for_openai() {
    let provider = ModelProviderInfo::create_openai_provider(/*base_url*/ None);

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_supports_remote_compaction_for_azure_name() {
    let provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://example.com/openai".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    };

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_supports_remote_compaction_for_non_openai_non_azure_provider() {
    let provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com/v1".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        aws: None,
        auth_header: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: false,
    };

    assert!(!provider.supports_remote_compaction());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".to_string()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn test_deserialize_provider_aws_config() {
    let provider_toml = r#"
name = "Amazon Bedrock"
base_url = "https://bedrock.example.com/v1"

[aws]
profile = "forall-bedrock"
region = "us-west-2"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();

    assert_eq!(
        provider.aws,
        Some(ModelProviderAwsAuthInfo {
            profile: Some("forall-bedrock".to_string()),
            region: Some("us-west-2".to_string()),
        })
    );
}

#[test]
fn test_create_amazon_bedrock_provider() {
    assert_eq!(
        ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
        ModelProviderInfo {
            name: "Amazon Bedrock".to_string(),
            base_url: Some("https://bedrock-mantle.us-east-1.api.aws/openai/v1".to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            auth_header: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: Some(maplit::hashmap! {
                AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER.to_string() =>
                    AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE.to_string(),
            }),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_openai_auth: false,
            requires_forall_auth: false,
            supports_websockets: false,
        }
    );
}

#[test]
fn test_amazon_bedrock_provider_adds_mantle_client_agent_header() {
    let api_provider = ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
        .to_api_provider(/*auth_mode*/ None)
        .expect("Amazon Bedrock provider should build API provider");

    assert_eq!(
        api_provider
            .headers
            .get(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(AMAZON_BEDROCK_MANTLE_CLIENT_AGENT_VALUE)
    );
}

#[test]
fn test_built_in_model_providers_include_amazon_bedrock() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    assert_eq!(
        providers
            .get(AMAZON_BEDROCK_PROVIDER_ID)
            .map(ModelProviderInfo::is_amazon_bedrock),
        Some(true)
    );
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_applies_amazon_bedrock_profile_override() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("forall-bedrock".to_string()),
                region: Some("us-west-2".to_string()),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected
        .get_mut(AMAZON_BEDROCK_PROVIDER_ID)
        .expect("Amazon Bedrock provider should be built in")
        .aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("forall-bedrock".to_string()),
        region: Some("us-west-2".to_string()),
    });

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_rejects_amazon_bedrock_non_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            name: "Custom Bedrock".to_string(),
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("forall-bedrock".to_string()),
                region: None,
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Err(
            "model_providers.amazon-bedrock only supports changing `aws.profile` and `aws.region`; other non-default provider fields are not supported"
                .to_string()
        )
    );
}

#[test]
fn test_merge_configured_model_providers_allows_amazon_bedrock_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        AMAZON_BEDROCK_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            }),
            wire_api: WireApi::Responses,
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(built_in_model_providers(/*openai_base_url*/ None))
    );
}

/// The Claude-on-Bedrock provider is registered separately from
/// `amazon-bedrock` and must not disturb it.
#[test]
fn built_in_providers_include_bedrock_anthropic_alongside_amazon_bedrock() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let bedrock_anthropic = providers
        .get(BEDROCK_ANTHROPIC_PROVIDER_ID)
        .expect("bedrock-anthropic provider should be built in");
    assert_eq!(bedrock_anthropic.wire_api, WireApi::BedrockMessages);
    assert_eq!(
        bedrock_anthropic.base_url.as_deref(),
        Some("https://bedrock-runtime.us-east-1.amazonaws.com")
    );
    assert!(bedrock_anthropic.is_bedrock_anthropic());
    assert!(!bedrock_anthropic.is_amazon_bedrock());
    assert!(!bedrock_anthropic.is_anthropic());
    // AWS-authenticated: no API key of any kind, and a bundled catalog rather
    // than a remote `/models` fetch.
    assert_eq!(bedrock_anthropic.env_key, None);
    assert_eq!(bedrock_anthropic.auth_header, None);
    assert!(bedrock_anthropic.aws.is_some());
    assert!(!bedrock_anthropic.uses_provider_model_catalog());
    assert!(!bedrock_anthropic.supports_websockets);
    assert_eq!(bedrock_anthropic.validate(), Ok(()));

    // The Mantle GPT provider is untouched.
    let amazon_bedrock = providers
        .get(AMAZON_BEDROCK_PROVIDER_ID)
        .expect("amazon-bedrock provider should be built in");
    assert_eq!(amazon_bedrock.wire_api, WireApi::Responses);
    assert!(!amazon_bedrock.is_bedrock_anthropic());
}

#[test]
fn test_merge_configured_model_providers_applies_bedrock_anthropic_aws_override() {
    let configured_model_providers = std::collections::HashMap::from([(
        BEDROCK_ANTHROPIC_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            aws: Some(ModelProviderAwsAuthInfo {
                profile: Some("forall-bedrock".to_string()),
                region: Some("eu-central-1".to_string()),
            }),
            ..ModelProviderInfo::default()
        },
    )]);

    let mut expected = built_in_model_providers(/*openai_base_url*/ None);
    expected
        .get_mut(BEDROCK_ANTHROPIC_PROVIDER_ID)
        .expect("bedrock-anthropic provider should be built in")
        .aws = Some(ModelProviderAwsAuthInfo {
        profile: Some("forall-bedrock".to_string()),
        region: Some("eu-central-1".to_string()),
    });

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Ok(expected)
    );
}

#[test]
fn test_merge_configured_model_providers_rejects_bedrock_anthropic_non_default_fields() {
    let configured_model_providers = std::collections::HashMap::from([(
        BEDROCK_ANTHROPIC_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            base_url: Some("https://not-bedrock.example".to_string()),
            ..ModelProviderInfo::default()
        },
    )]);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*openai_base_url*/ None),
            configured_model_providers,
        ),
        Err(
            "model_providers.bedrock-anthropic only supports changing `aws.profile` and `aws.region`; other non-default provider fields are not supported"
                .to_string()
        )
    );
}

#[test]
fn built_in_providers_include_openrouter_and_litellm() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let openrouter = providers
        .get(OPENROUTER_PROVIDER_ID)
        .expect("openrouter provider");
    assert_eq!(openrouter.name, "OpenRouter");
    assert_eq!(
        openrouter.base_url.as_deref(),
        Some("https://openrouter.ai/api/v1")
    );
    assert_eq!(openrouter.env_key.as_deref(), Some("OPENROUTER_API_KEY"));
    assert!(!openrouter.requires_openai_auth);
    assert_eq!(openrouter.wire_api, WireApi::Responses);

    let litellm = providers
        .get(LITELLM_PROVIDER_ID)
        .expect("litellm provider");
    assert_eq!(litellm.name, "LiteLLM");
    assert_eq!(
        litellm.base_url.as_deref(),
        Some("http://localhost:4000/v1")
    );
    assert!(litellm.env_key.is_none());
    assert!(!litellm.requires_openai_auth);
    assert_eq!(litellm.wire_api, WireApi::Responses);
    assert!(openrouter.uses_provider_model_catalog());
    assert!(litellm.uses_provider_model_catalog());

    let openai = providers.get(OPENAI_PROVIDER_ID).expect("openai provider");
    assert!(!openai.uses_provider_model_catalog());

    assert_eq!(
        builtin_provider_profile_id(OPENROUTER_PROVIDER_ID).as_deref(),
        Some(OPENROUTER_PROVIDER_ID)
    );
    assert_eq!(
        resolve_effective_model_provider_id(None, None, Some(OPENROUTER_PROVIDER_ID), true),
        OPENROUTER_PROVIDER_ID
    );
}

#[test]
fn built_in_providers_include_astrio() {
    let providers = built_in_model_providers(/*openai_base_url*/ None);

    let astrio = providers.get(ASTRIO_PROVIDER_ID).expect("astrio provider");
    assert_eq!(astrio.name, "Forall");
    assert_eq!(
        astrio.base_url.as_deref(),
        Some("https://api.forall.astrio.app/v1")
    );
    assert!(astrio.env_key.is_none());
    assert!(astrio.experimental_bearer_token.is_none());
    assert!(astrio.auth.is_none());
    assert!(astrio.aws.is_none());
    assert_eq!(astrio.wire_api, WireApi::Responses);
    assert!(!astrio.requires_openai_auth);
    assert!(astrio.requires_forall_auth);
    assert!(astrio.uses_provider_model_catalog());

    assert_eq!(
        builtin_provider_profile_id(ASTRIO_PROVIDER_ID).as_deref(),
        Some(ASTRIO_PROVIDER_ID)
    );
    assert_eq!(
        resolve_effective_model_provider_id(None, None, Some(ASTRIO_PROVIDER_ID), true),
        ASTRIO_PROVIDER_ID
    );
}

#[test]
fn built_in_providers_include_azure() {
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn clear(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation restored on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restore env for other tests.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // Without AZURE_OPENAI_BASE_URL, the provider is still registered but has
    // no base URL and is rejected at use-time instead of silently pointing at
    // the OpenAI default base URL.
    {
        let _base_guard = EnvGuard::clear(AZURE_OPENAI_BASE_URL_ENV_VAR);
        let _version_guard = EnvGuard::clear(AZURE_OPENAI_API_VERSION_ENV_VAR);

        let providers = built_in_model_providers(/*openai_base_url*/ None);
        let azure = providers.get(AZURE_PROVIDER_ID).expect("azure provider");
        assert_eq!(azure.name, "Azure OpenAI");
        assert_eq!(azure.base_url, None);
        assert_eq!(azure.env_key.as_deref(), Some("AZURE_OPENAI_API_KEY"));
        assert_eq!(azure.auth_header.as_deref(), Some("api-key"));
        assert_eq!(azure.wire_api, WireApi::Responses);
        assert_eq!(
            azure
                .query_params
                .as_ref()
                .and_then(|params| params.get("api-version"))
                .map(String::as_str),
            Some("preview")
        );
        assert!(!azure.requires_openai_auth);
        assert!(!azure.requires_forall_auth);
        assert!(azure.aws.is_none());
        assert!(azure.supports_remote_compaction());
        assert_eq!(azure.validate(), Ok(()));

        let err = azure
            .to_api_provider(/*auth_mode*/ None)
            .expect_err("azure without a base URL must be rejected at use-time");
        assert!(err.to_string().contains(AZURE_OPENAI_BASE_URL_ENV_VAR));
    }

    // With a base URL configured, the provider builds normally. Constructed
    // via struct update instead of setting AZURE_OPENAI_BASE_URL so this test
    // never races other tests that snapshot `built_in_model_providers()`.
    {
        let _base_guard = EnvGuard::clear(AZURE_OPENAI_BASE_URL_ENV_VAR);
        let _version_guard = EnvGuard::clear(AZURE_OPENAI_API_VERSION_ENV_VAR);

        let azure = ModelProviderInfo {
            base_url: Some("https://my-resource.openai.azure.com/openai/v1".to_string()),
            ..ModelProviderInfo::create_azure_provider()
        };
        assert!(azure.uses_provider_model_catalog());

        let api_provider = azure
            .to_api_provider(/*auth_mode*/ None)
            .expect("azure with a base URL should build an API provider");
        assert_eq!(
            api_provider.base_url,
            "https://my-resource.openai.azure.com/openai/v1"
        );
        assert!(api_provider.is_azure_responses_endpoint());
        assert_eq!(
            api_provider
                .query_params
                .as_ref()
                .and_then(|params| params.get("api-version"))
                .map(String::as_str),
            Some("preview")
        );
    }
}

#[test]
fn built_in_providers_include_anthropic() {
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn clear(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation restored on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restore env for other tests.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    let _base_guard = EnvGuard::clear(ANTHROPIC_BASE_URL_ENV_VAR);

    let providers = built_in_model_providers(/*openai_base_url*/ None);
    let anthropic = providers
        .get(ANTHROPIC_PROVIDER_ID)
        .expect("anthropic provider");
    assert_eq!(anthropic.name, "Anthropic");
    assert!(anthropic.is_anthropic());
    assert_eq!(
        anthropic.base_url.as_deref(),
        Some("https://api.anthropic.com/v1")
    );
    assert_eq!(anthropic.env_key.as_deref(), Some("ANTHROPIC_API_KEY"));
    assert_eq!(anthropic.auth_header.as_deref(), Some("x-api-key"));
    assert_eq!(anthropic.wire_api, WireApi::Messages);
    assert_eq!(
        anthropic
            .http_headers
            .as_ref()
            .and_then(|headers| headers.get("anthropic-version"))
            .map(String::as_str),
        Some("2023-06-01")
    );
    assert!(!anthropic.requires_openai_auth);
    assert!(!anthropic.requires_forall_auth);
    assert!(!anthropic.supports_websockets);
    assert!(anthropic.aws.is_none());
    assert_eq!(anthropic.validate(), Ok(()));
    // Messages providers ship a bundled catalog instead of a remote `/models`
    // endpoint, so the OpenAI-compatible catalog path must stay off.
    assert!(!anthropic.uses_provider_model_catalog());
    assert!(!anthropic.supports_remote_compaction());

    let api_provider = anthropic
        .to_api_provider(/*auth_mode*/ None)
        .expect("anthropic provider should build API provider");
    assert_eq!(api_provider.base_url, "https://api.anthropic.com/v1");
    assert_eq!(
        api_provider
            .headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok()),
        Some("2023-06-01")
    );
}

#[test]
fn built_in_providers_include_gemini() {
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn clear(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation restored on drop.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restore env for other tests.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    let _base_guard = EnvGuard::clear(GEMINI_BASE_URL_ENV_VAR);

    let providers = built_in_model_providers(/*openai_base_url*/ None);
    let gemini = providers.get(GEMINI_PROVIDER_ID).expect("gemini provider");
    assert_eq!(gemini.name, "Google Gemini");
    assert!(gemini.is_gemini());
    // The base URL carries the API version segment because Gemini paths are
    // model-scoped and version-free.
    assert_eq!(
        gemini.base_url.as_deref(),
        Some("https://generativelanguage.googleapis.com/v1beta")
    );
    assert_eq!(gemini.env_key.as_deref(), Some("GEMINI_API_KEY"));
    assert_eq!(gemini.auth_header.as_deref(), Some("x-goog-api-key"));
    assert_eq!(gemini.wire_api, WireApi::Gemini);
    // `alt=sse` is appended by the endpoint, never configured here, so a user
    // query string cannot displace it.
    assert_eq!(gemini.query_params, None);
    assert!(!gemini.requires_openai_auth);
    assert!(!gemini.requires_forall_auth);
    assert!(!gemini.supports_websockets);
    assert!(gemini.aws.is_none());
    assert_eq!(gemini.validate(), Ok(()));
    // Gemini ships a bundled catalog instead of an OpenAI-compatible `/models`
    // endpoint, so the remote catalog path must stay off.
    assert!(!gemini.uses_provider_model_catalog());
    assert!(!gemini.supports_remote_compaction());

    let api_provider = gemini
        .to_api_provider(/*auth_mode*/ None)
        .expect("gemini provider should build API provider");
    assert_eq!(
        api_provider.base_url,
        "https://generativelanguage.googleapis.com/v1beta"
    );
    // Gemini's model-scoped path composes onto the versioned base URL.
    assert_eq!(
        api_provider.url_for_path("models/gemini-2.5-pro:streamGenerateContent"),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-pro:streamGenerateContent"
    );
}

#[test]
fn test_deserialize_gemini_wire_api() {
    let provider_toml = r#"
name = "Custom Gemini"
base_url = "https://example.com/v1beta"
env_key = "GEMINI_API_KEY"
auth_header = "x-goog-api-key"
wire_api = "gemini"
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::Gemini);
    assert_eq!(provider.validate(), Ok(()));
}

/// `Display` and `Deserialize` are both hand-written for `WireApi` and
/// `Serialize` is derived with per-variant renames, so all three can drift
/// apart; every variant must round-trip through its wire string.
#[test]
fn test_wire_api_display_matches_deserialize_for_every_variant() {
    for (wire_api, expected) in [
        (WireApi::Responses, "responses"),
        (WireApi::Messages, "messages"),
        (WireApi::Gemini, "gemini"),
        (WireApi::BedrockMessages, "bedrock_messages"),
    ] {
        assert_eq!(wire_api.to_string(), expected);
        assert_eq!(
            toml::Value::try_from(wire_api).expect("serialize wire api"),
            toml::Value::String(expected.to_string())
        );

        let provider_toml = format!("name = \"x\"\nwire_api = \"{expected}\"\n");
        let provider: ModelProviderInfo =
            toml::from_str(&provider_toml).expect("deserialize wire api");
        assert_eq!(provider.wire_api, wire_api);
    }
}

#[test]
fn test_deserialize_unknown_wire_api_lists_every_variant() {
    let provider_toml = r#"
name = "Bogus"
wire_api = "bogus"
        "#;

    let err = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap_err();
    let message = err.to_string();
    for expected in ["responses", "messages", "gemini", "bedrock_messages"] {
        assert!(
            message.contains(expected),
            "expected-variants list should mention {expected}: {message}"
        );
    }
}

#[test]
fn test_validate_auth_header_rejects_aws_combination() {
    let provider = ModelProviderInfo {
        auth_header: Some("api-key".to_string()),
        ..ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with auth_header".to_string())
    );
}

#[test]
fn test_validate_auth_header_rejects_requires_forall_auth() {
    let provider = ModelProviderInfo {
        auth_header: Some("api-key".to_string()),
        ..ModelProviderInfo::create_astrio_provider()
    };

    assert_eq!(
        provider.validate(),
        Err("provider auth_header cannot be combined with requires_forall_auth".to_string())
    );
}

#[test]
fn test_validate_auth_header_rejects_invalid_header_name() {
    let provider = ModelProviderInfo {
        auth_header: Some("not a header".to_string()),
        ..ModelProviderInfo::create_openrouter_provider()
    };

    assert_eq!(
        provider.validate(),
        Err("provider auth_header is not a valid HTTP header name: `not a header`".to_string())
    );
}

#[test]
fn config_toml_providers_default_requires_forall_auth_to_false() {
    let provider: ModelProviderInfo = toml::from_str(
        r#"
        name = "Custom"
        base_url = "https://example.com/v1"
        "#,
    )
    .expect("provider should deserialize without requires_forall_auth");

    assert!(!provider.requires_forall_auth);
}

#[test]
fn resolve_effective_model_provider_id_skips_env_inference_until_byok_onboarding() {
    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation.
            unsafe { std::env::set_var(key, value) };
            Self { key, original }
        }

        fn clear(key: &'static str) -> Self {
            let original = std::env::var(key).ok();
            // SAFETY: test-only env mutation.
            unsafe { std::env::remove_var(key) };
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restore env for other tests.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    let _openai_guard = EnvGuard::clear("OPENAI_API_KEY");
    let _openrouter_guard = EnvGuard::set("OPENROUTER_API_KEY", "sk-or-test");
    assert_eq!(
        resolve_effective_model_provider_id(None, None, None, false),
        OPENAI_PROVIDER_ID
    );
    assert_eq!(
        resolve_effective_model_provider_id(None, None, None, true),
        OPENROUTER_PROVIDER_ID
    );
}

#[test]
fn test_validate_provider_aws_rejects_conflicting_auth() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        env_key: Some("AWS_BEARER_TOKEN_BEDROCK".to_string()),
        supports_websockets: false,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with env_key, requires_openai_auth".to_string())
    );
}

#[test]
fn test_validate_provider_aws_rejects_websockets() {
    let provider = ModelProviderInfo {
        aws: Some(ModelProviderAwsAuthInfo {
            profile: None,
            region: None,
        }),
        requires_openai_auth: false,
        requires_forall_auth: false,
        supports_websockets: true,
        ..ModelProviderInfo::create_openai_provider(/*base_url*/ None)
    };

    assert_eq!(
        provider.validate(),
        Err("provider aws cannot be combined with supports_websockets".to_string())
    );
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}

#[test]
fn configured_azure_provider_overlays_the_built_in() {
    // Regression (PR #78 review): registering azure as a built-in made the
    // `or_insert` merge discard a user's `[model_providers.azure]` entry, so
    // their endpoint was ignored. The user's explicit fields must win while
    // the built-in's api-key header and api-version survive.
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            base_url: Some("https://my-resource.openai.azure.com/openai/v1".to_string()),
            ..Default::default()
        },
    )]);

    let merged = merge_configured_model_providers(built_ins, configured)
        .expect("azure overlay should merge");
    let azure = merged
        .get(AZURE_PROVIDER_ID)
        .expect("azure provider present");

    assert_eq!(
        azure.base_url.as_deref(),
        Some("https://my-resource.openai.azure.com/openai/v1"),
        "the user's endpoint must win"
    );
    assert_eq!(
        azure.auth_header.as_deref(),
        Some("api-key"),
        "the built-in's named auth header must survive, or the key goes out as Bearer"
    );
    assert!(
        azure
            .query_params
            .as_ref()
            .is_some_and(|params| params.contains_key("api-version")),
        "the built-in's api-version must survive"
    );
}

#[test]
fn configured_azure_provider_can_override_built_in_fields() {
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            env_key: Some("MY_AZURE_KEY".to_string()),
            auth_header: Some("x-custom-key".to_string()),
            ..Default::default()
        },
    )]);

    let merged = merge_configured_model_providers(built_ins, configured)
        .expect("azure overlay should merge");
    let azure = merged
        .get(AZURE_PROVIDER_ID)
        .expect("azure provider present");

    assert_eq!(azure.env_key.as_deref(), Some("MY_AZURE_KEY"));
    assert_eq!(azure.auth_header.as_deref(), Some("x-custom-key"));
}

#[test]
fn configured_provider_cannot_grant_itself_forall_auth() {
    // A remote/user-supplied provider must never claim the Forall OAuth
    // token; only built-ins may set this.
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            requires_forall_auth: true,
            ..Default::default()
        },
    )]);

    let merged = merge_configured_model_providers(built_ins, configured)
        .expect("azure overlay should merge");
    assert!(
        !merged
            .get(AZURE_PROVIDER_ID)
            .expect("azure provider present")
            .requires_forall_auth,
        "config must not be able to grant Forall-account auth"
    );
}

#[test]
fn configured_azure_bearer_token_displaces_the_built_in_env_key() {
    // Regression (PR #78 review): the resolver short-circuits on a present
    // `env_key` and returns unauthenticated when that variable is unset, so
    // retaining the built-in's AZURE_OPENAI_API_KEY slot would shadow a
    // configured bearer token entirely.
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            experimental_bearer_token: Some("token-abc".to_string()),
            ..Default::default()
        },
    )]);

    let merged =
        merge_configured_model_providers(built_ins, configured).expect("azure overlay merges");
    let azure = merged
        .get(AZURE_PROVIDER_ID)
        .expect("azure provider present");

    assert_eq!(
        azure.experimental_bearer_token.as_deref(),
        Some("token-abc")
    );
    assert!(
        azure.env_key.is_none(),
        "built-in env_key must not shadow the configured bearer token"
    );
    // The rest of the built-in survives.
    assert_eq!(azure.auth_header.as_deref(), Some("api-key"));
}

#[test]
fn configured_azure_command_auth_displaces_the_built_in_env_key() {
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            auth: Some(ModelProviderAuthInfo {
                command: "token-helper".to_string(),
                args: Vec::new(),
                timeout_ms: NonZeroU64::new(5_000).expect("non-zero"),
                refresh_interval_ms: 300_000,
                cwd: AbsolutePathBuf::current_dir().expect("cwd"),
            }),
            ..Default::default()
        },
    )]);

    let merged =
        merge_configured_model_providers(built_ins, configured).expect("azure overlay merges");
    let azure = merged
        .get(AZURE_PROVIDER_ID)
        .expect("azure provider present");

    assert!(azure.auth.is_some(), "configured command auth must survive");
    assert!(
        azure.env_key.is_none(),
        "built-in env_key must not shadow command-backed auth"
    );
}

#[test]
fn configured_azure_env_key_keeps_working_alongside_the_built_in_header() {
    // The common case must be unaffected by the mechanism-displacement logic.
    let built_ins = built_in_model_providers(/*forall_auth_base_url*/ None);
    let configured = HashMap::from([(
        AZURE_PROVIDER_ID.to_string(),
        ModelProviderInfo {
            env_key: Some("MY_AZURE_KEY".to_string()),
            ..Default::default()
        },
    )]);

    let merged =
        merge_configured_model_providers(built_ins, configured).expect("azure overlay merges");
    let azure = merged
        .get(AZURE_PROVIDER_ID)
        .expect("azure provider present");

    assert_eq!(azure.env_key.as_deref(), Some("MY_AZURE_KEY"));
    assert_eq!(azure.auth_header.as_deref(), Some("api-key"));
    assert!(azure.experimental_bearer_token.is_none());
}
