use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_forall::test_forall;
use core_test_support::wait_for_event;
use forall_core::config::Config;
use forall_extension_api::ExtensionRegistry;
use forall_extension_api::ExtensionRegistryBuilder;
use forall_features::Feature;
use forall_image_generation_extension::install as install_image_generation_extension;
use forall_login::ForallAuth;
use forall_protocol::config_types::WebSearchMode;
use forall_protocol::models::ImageDetail;
use forall_protocol::openai_models::InputModality;
use forall_protocol::protocol::EventMsg;
use forall_protocol::protocol::Op;
use forall_protocol::user_input::UserInput;
use forall_web_search_extension::install as install_web_search_extension;
use pretty_assertions::assert_eq;
use serde_json::Value;

const RESPONSES_LITE_HEADER: &str = "x-openai-internal-forall-responses-lite";

fn responses_extensions(auth: &ForallAuth) -> Arc<ExtensionRegistry<Config>> {
    let auth_manager = forall_core::test_support::auth_manager_from_auth(auth.clone());
    let mut extension_builder = ExtensionRegistryBuilder::<Config>::new();
    install_web_search_extension(&mut extension_builder, Arc::clone(&auth_manager));
    install_image_generation_extension(&mut extension_builder, auth_manager);
    Arc::new(extension_builder.build())
}

fn configure_responses_tools(config: &mut Config) {
    assert!(config.web_search_mode.set(WebSearchMode::Live).is_ok());
    assert!(
        config
            .features
            .disable(Feature::StandaloneWebSearch)
            .is_ok()
    );
    assert!(config.features.enable(Feature::ImageGeneration).is_ok());
    assert!(config.features.disable(Feature::ImageGenExt).is_ok());
}

fn configure_image_capable_model(model_info: &mut forall_protocol::openai_models::ModelInfo) {
    model_info.input_modalities = vec![InputModality::Text, InputModality::Image];
}

fn has_hosted_tool(tools: &[Value], tool_type: &str) -> bool {
    tools
        .iter()
        .any(|tool| tool.get("type").and_then(Value::as_str) == Some(tool_type))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_prepares_images() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==";
    let remote_image_url = "https://example.com/image.png";
    let mut builder = test_forall().with_model_info_override("gpt-5.4", |model_info| {
        model_info.use_responses_lite = true;
        configure_image_capable_model(model_info);
    });
    let test = builder.build(&server).await?;

    test.forall
        .submit(Op::UserInput {
            items: vec![
                UserInput::Image {
                    image_url: image_url.to_string(),
                    detail: Some(ImageDetail::Original),
                },
                UserInput::Image {
                    image_url: remote_image_url.to_string(),
                    detail: Some(ImageDetail::High),
                },
            ],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&test.forall, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = response_mock.single_request();
    let user_content = request
        .input()
        .into_iter()
        .rev()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|item| item.get("content").and_then(Value::as_array).cloned())
        .context("request should contain user content")?;
    assert_eq!(
        user_content,
        vec![
            serde_json::json!({
                "type": "input_image",
                "image_url": image_url
            }),
            serde_json::json!({
                "type": "input_text",
                "text": "image content omitted because remote image URLs are not supported"
            }),
        ]
    );
    assert!(!request.body_json().to_string().contains(remote_image_url));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
async fn responses_lite_uses_standalone_web_search_and_image_generation() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let auth = ForallAuth::create_dummy_api_key_auth_for_testing();
    let extensions = responses_extensions(&auth);

    let mut builder = test_forall()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Use standalone tools").await?;

    let request = response_mock.single_request();
    assert_eq!(
        request.header(RESPONSES_LITE_HEADER).as_deref(),
        Some("true")
    );
    request
        .tool_by_name("web", "run")
        .context("Responses Lite should expose standalone web search")?;
    request
        .tool_by_name("image_gen", "imagegen")
        .context("Responses Lite should expose standalone image generation")?;

    let body = request.body_json();
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(!has_hosted_tool(tools, "web_search"));
    assert!(!has_hosted_tool(tools, "image_generation"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_compact_request_uses_lite_transport_contract() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let compact_mock =
        responses::mount_compact_json_once(&server, serde_json::json!({ "output": [] })).await;

    let mut builder = test_forall()
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            model_info.supports_parallel_tool_calls = true;
        })
        .with_config(|config| {
            let _ = config.features.disable(Feature::RemoteCompactionV2);
        });
    let test = builder.build(&server).await?;

    test.submit_turn("Compact this conversation").await?;
    test.forall.submit(Op::Compact).await?;
    wait_for_event(&test.forall, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    response_mock.single_request();
    let compact_request = compact_mock.single_request();
    assert_eq!(
        compact_request.header(RESPONSES_LITE_HEADER).as_deref(),
        Some("true")
    );
    let compact_body = compact_request.body_json();
    assert_eq!(
        compact_body
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("context"))
            .and_then(Value::as_str),
        Some("all_turns")
    );
    assert_eq!(
        compact_body.get("parallel_tool_calls"),
        Some(&Value::Bool(false))
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_omits_hosted_tools_without_standalone_extensions() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let mut builder = test_forall()
        .with_auth(ForallAuth::create_dummy_api_key_auth_for_testing())
        .with_model_info_override("gpt-5.4", |model_info| {
            model_info.use_responses_lite = true;
            configure_image_capable_model(model_info);
        })
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Do not use hosted tools").await?;

    let body = response_mock.single_request().body_json();
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(!has_hosted_tool(tools, "web_search"));
    assert!(!has_hosted_tool(tools, "image_generation"));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
async fn non_lite_uses_hosted_tools_when_standalone_features_are_disabled() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;

    let auth = ForallAuth::create_dummy_api_key_auth_for_testing();
    let extensions = responses_extensions(&auth);
    let mut builder = test_forall()
        .with_auth(auth)
        .with_extensions(extensions)
        .with_model_info_override("gpt-5.4", configure_image_capable_model)
        .with_config(configure_responses_tools);
    let test = builder.build(&server).await?;

    test.submit_turn("Use hosted tools").await?;

    let request = response_mock.single_request();
    assert_eq!(request.header(RESPONSES_LITE_HEADER), None);
    assert!(request.tool_by_name("web", "run").is_none());
    assert!(request.tool_by_name("image_gen", "imagegen").is_none());
    let body = request.body_json();
    let tools = body["tools"]
        .as_array()
        .context("Responses request tools should be an array")?;
    assert!(has_hosted_tool(tools, "web_search"));
    assert!(has_hosted_tool(tools, "image_generation"));

    Ok(())
}
