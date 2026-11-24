use std::time::Duration;

use anyhow::Result;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_repeating_assistant;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use forall_app_server_protocol::ConfigReadParams;
use forall_app_server_protocol::ConfigReadResponse;
use forall_app_server_protocol::ExperimentalFeature;
use forall_app_server_protocol::ExperimentalFeatureEnablementSetParams;
use forall_app_server_protocol::ExperimentalFeatureEnablementSetResponse;
use forall_app_server_protocol::ExperimentalFeatureListParams;
use forall_app_server_protocol::ExperimentalFeatureListResponse;
use forall_app_server_protocol::ExperimentalFeatureStage;
use forall_app_server_protocol::JSONRPCError;
use forall_app_server_protocol::JSONRPCResponse;
use forall_app_server_protocol::RequestId;
use forall_app_server_protocol::ThreadStartParams;
use forall_app_server_protocol::ThreadStartResponse;
use forall_config::LoaderOverrides;
use forall_config::types::AuthCredentialsStoreMode;
use forall_core::config::ConfigBuilder;
use forall_core::config::set_project_trust_level;
use forall_features::FEATURES;
use forall_features::Stage;
use forall_protocol::config_types::TrustLevel;
use pretty_assertions::assert_eq;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::BTreeMap;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn experimental_feature_list_returns_feature_metadata_with_stage() -> Result<()> {
    let forall_home = TempDir::new()?;
    let config = ConfigBuilder::default()
        .forall_home(forall_home.path().to_path_buf())
        .fallback_cwd(Some(forall_home.path().to_path_buf()))
        .loader_overrides(LoaderOverrides::with_managed_config_path_for_tests(
            forall_home.path().join("managed_config.toml"),
        ))
        .build()
        .await?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;

    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_experimental_feature_list_request(ExperimentalFeatureListParams::default())
        .await?;

    let actual = read_response::<ExperimentalFeatureListResponse>(&mut mcp, request_id).await?;
    let expected_data = FEATURES
        .iter()
        .map(|spec| {
            let (stage, display_name, description, announcement) = match spec.stage {
                Stage::Experimental {
                    name,
                    menu_description,
                    announcement,
                } => (
                    ExperimentalFeatureStage::Beta,
                    Some(name.to_string()),
                    Some(menu_description.to_string()),
                    Some(announcement.to_string()),
                ),
                Stage::UnderDevelopment => {
                    (ExperimentalFeatureStage::UnderDevelopment, None, None, None)
                }
                Stage::Stable => (ExperimentalFeatureStage::Stable, None, None, None),
                Stage::Deprecated => (ExperimentalFeatureStage::Deprecated, None, None, None),
                Stage::Removed => (ExperimentalFeatureStage::Removed, None, None, None),
            };

            ExperimentalFeature {
                name: spec.key.to_string(),
                stage,
                display_name,
                description,
                announcement,
                enabled: config.features.enabled(spec.id),
                default_enabled: spec.default_enabled,
            }
        })
        .collect::<Vec<_>>();
    let expected = ExperimentalFeatureListResponse {
        data: expected_data,
        next_cursor: None,
    };

    assert_eq!(actual, expected);
    Ok(())
}

#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
#[tokio::test]
async fn experimental_feature_list_marks_apps_and_plugins_disabled_by_workspace_policy()
-> Result<()> {
    let forall_home = TempDir::new()?;
    let server = MockServer::start().await;
    std::fs::write(
        forall_home.path().join("config.toml"),
        r#"[features]
plugins = true
"#,
    )?;
    write_api_key_auth(
        forall_home.path(),
        ApiKeyAuthFixture::new("chatgpt-token")
            .account_id("account-123")
            .chatgpt_user_id("user-123")
            .chatgpt_account_id("account-123")
            .plan_type("team"),
        AuthCredentialsStoreMode::File,
    )?;
    Mock::given(method("GET"))
        .and(path("/backend-api/accounts/account-123/settings"))
        .and(header("authorization", "Bearer chatgpt-token"))
        .and(header("chatgpt-account-id", "account-123"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"beta_settings":{"enable_plugins":false}}"#),
        )
        .mount(&server)
        .await;

    let mut mcp = TestAppServer::new_without_managed_config(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_experimental_feature_list_request(ExperimentalFeatureListParams::default())
        .await?;

    let actual = read_response::<ExperimentalFeatureListResponse>(&mut mcp, request_id).await?;
    let apps = actual
        .data
        .iter()
        .find(|feature| feature.name == "apps")
        .expect("apps feature should be present");
    let plugins = actual
        .data
        .iter()
        .find(|feature| feature.name == "plugins")
        .expect("plugins feature should be present");
    assert!(!apps.enabled);
    assert!(!plugins.enabled);
    assert!(apps.default_enabled);
    assert!(plugins.default_enabled);
    Ok(())
}

#[tokio::test]
async fn experimental_feature_list_resolves_thread_project_config() -> Result<()> {
    let server = create_mock_responses_server_repeating_assistant("Done").await;
    let forall_home = TempDir::new()?;
    let workspace = TempDir::new()?;
    let server_uri = server.uri();
    std::fs::write(
        forall_home.path().join("config.toml"),
        format!(
            r#"model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"
model_provider = "mock_provider"

[features]
memories = true

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )?;
    set_project_trust_level(forall_home.path(), workspace.path(), TrustLevel::Trusted)?;

    let mut mcp = TestAppServer::new_without_managed_config(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let thread_start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            cwd: Some(workspace.path().display().to_string()),
            ..Default::default()
        })
        .await?;
    let ThreadStartResponse { thread, .. } =
        read_response::<ThreadStartResponse>(&mut mcp, thread_start_id).await?;

    let request_id = mcp
        .send_experimental_feature_list_request(ExperimentalFeatureListParams {
            cursor: None,
            limit: None,
            thread_id: Some(thread.id),
        })
        .await?;

    let actual = read_response::<ExperimentalFeatureListResponse>(&mut mcp, request_id).await?;
    let memories = actual
        .data
        .iter()
        .find(|feature| feature.name == "memories")
        .expect("memories feature should be present");
    assert!(memories.enabled);

    Ok(())
}

#[tokio::test]
async fn experimental_feature_list_rejects_unknown_thread_id() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_experimental_feature_list_request(ExperimentalFeatureListParams {
            cursor: None,
            limit: None,
            thread_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
        })
        .await?;
    let JSONRPCError { error, .. } = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.code, -32600);
    assert!(
        error
            .message
            .contains("thread not found: 00000000-0000-4000-8000-000000000001"),
        "{}",
        error.message
    );

    Ok(())
}

#[tokio::test]
async fn experimental_feature_enablement_set_applies_to_global_and_thread_config_reads()
-> Result<()> {
    let forall_home = TempDir::new()?;
    let project_cwd = forall_home.path().join("project");
    std::fs::create_dir_all(&project_cwd)?;

    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let actual = set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([("auth_elicitation".to_string(), true)]),
    )
    .await?;
    assert_eq!(
        actual,
        ExperimentalFeatureEnablementSetResponse {
            enablement: BTreeMap::from([("auth_elicitation".to_string(), true)]),
        }
    );

    for cwd in [None, Some(project_cwd.display().to_string())] {
        let ConfigReadResponse { config, .. } = read_config(&mut mcp, cwd).await?;

        assert_eq!(
            config
                .additional
                .get("features")
                .and_then(|features| features.get("auth_elicitation")),
            Some(&json!(true))
        );
    }

    Ok(())
}

#[tokio::test]
async fn experimental_feature_enablement_set_does_not_override_user_config() -> Result<()> {
    let forall_home = TempDir::new()?;
    std::fs::write(
        forall_home.path().join("config.toml"),
        "[features]\nmemories = false\n",
    )?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let actual = set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([("memories".to_string(), true)]),
    )
    .await?;
    assert_eq!(
        actual,
        ExperimentalFeatureEnablementSetResponse {
            enablement: BTreeMap::from([("memories".to_string(), true)]),
        }
    );

    let ConfigReadResponse { config, .. } = read_config(&mut mcp, /*cwd*/ None).await?;

    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("memories")),
        Some(&json!(false))
    );

    Ok(())
}

#[tokio::test]
async fn experimental_feature_enablement_set_only_updates_named_features() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([("mentions_v2".to_string(), true)]),
    )
    .await?;
    let actual = set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([
            ("auth_elicitation".to_string(), true),
            ("memories".to_string(), true),
            ("remote_plugin".to_string(), true),
            ("tool_suggest".to_string(), false),
        ]),
    )
    .await?;

    assert_eq!(
        actual,
        ExperimentalFeatureEnablementSetResponse {
            enablement: BTreeMap::from([
                ("auth_elicitation".to_string(), true),
                ("memories".to_string(), true),
                ("remote_plugin".to_string(), true),
                ("tool_suggest".to_string(), false),
            ]),
        }
    );

    let ConfigReadResponse { config, .. } = read_config(&mut mcp, /*cwd*/ None).await?;

    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("mentions_v2")),
        Some(&json!(true))
    );
    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("auth_elicitation")),
        Some(&json!(true))
    );
    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("memories")),
        Some(&json!(true))
    );
    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("remote_plugin")),
        Some(&json!(true))
    );
    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("tool_suggest")),
        Some(&json!(false))
    );

    Ok(())
}

#[tokio::test]
async fn experimental_feature_enablement_set_empty_map_is_no_op() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([("mentions_v2".to_string(), true)]),
    )
    .await?;
    let actual = set_experimental_feature_enablement(&mut mcp, BTreeMap::new()).await?;

    assert_eq!(
        actual,
        ExperimentalFeatureEnablementSetResponse {
            enablement: BTreeMap::new(),
        }
    );

    let ConfigReadResponse { config, .. } = read_config(&mut mcp, /*cwd*/ None).await?;

    assert_eq!(
        config
            .additional
            .get("features")
            .and_then(|features| features.get("mentions_v2")),
        Some(&json!(true))
    );

    Ok(())
}

#[tokio::test]
async fn experimental_feature_enablement_set_ignores_invalid_features() -> Result<()> {
    let forall_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(forall_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let actual = set_experimental_feature_enablement(
        &mut mcp,
        BTreeMap::from([
            ("apps".to_string(), false),
            ("auth_elicitation".to_string(), true),
            ("connectors".to_string(), false),
            ("personality".to_string(), false),
            ("plugins".to_string(), false),
            ("tool_call_mcp_elicitation".to_string(), false),
            ("unknown_feature".to_string(), true),
        ]),
    )
    .await?;

    assert_eq!(
        actual,
        ExperimentalFeatureEnablementSetResponse {
            enablement: BTreeMap::from([("auth_elicitation".to_string(), true)]),
        }
    );

    Ok(())
}

async fn set_experimental_feature_enablement(
    mcp: &mut TestAppServer,
    enablement: BTreeMap<String, bool>,
) -> Result<ExperimentalFeatureEnablementSetResponse> {
    let request_id = mcp
        .send_experimental_feature_enablement_set_request(ExperimentalFeatureEnablementSetParams {
            enablement,
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn read_config(mcp: &mut TestAppServer, cwd: Option<String>) -> Result<ConfigReadResponse> {
    let request_id = mcp
        .send_config_read_request(ConfigReadParams {
            include_layers: false,
            cwd,
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn read_response<T: DeserializeOwned>(mcp: &mut TestAppServer, request_id: i64) -> Result<T> {
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(response)
}
