#![cfg(not(target_os = "windows"))]

use core_test_support::test_forall::local_selections;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::ev_shell_command_call;
use core_test_support::responses::sse;
use core_test_support::responses::sse_response;
use core_test_support::skip_if_no_network;
use core_test_support::test_forall::test_forall;
use core_test_support::test_forall::turn_permission_fields;
use core_test_support::wait_for_event_with_timeout;
use forall_features::Feature;
use forall_login::ForallAuth;
use forall_protocol::models::PermissionProfile;
use forall_protocol::openai_models::ModelsResponse;
use forall_protocol::protocol::AskForApproval;
use forall_protocol::protocol::EventMsg;
use forall_protocol::protocol::Op;
use forall_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use wiremock::MockServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
async fn refresh_models_on_models_etag_mismatch_and_avoid_duplicate_models_fetch() -> Result<()> {
    skip_if_no_network!(Ok(()));

    const ETAG_1: &str = "\"models-etag-1\"";
    const ETAG_2: &str = "\"models-etag-2\"";
    const CALL_ID: &str = "shell-command-call-1";

    let server = MockServer::start().await;

    // 1) On spawn, Forall fetches /models and stores the ETag.
    let spawn_models_mock = responses::mount_models_once_with_etag(
        &server,
        ModelsResponse { models: Vec::new() },
        ETAG_1,
    )
    .await;

    let auth = ForallAuth::create_dummy_api_key_auth_for_testing();
    let mut builder = test_forall()
        .with_auth(auth)
        .with_model("gpt-5.2")
        .with_config(|config| {
            // Keep this test deterministic: no request retries, and a small stream retry budget.
            config.model_provider.request_max_retries = Some(0);
            config.model_provider.stream_max_retries = Some(1);
            config
                .features
                .disable(Feature::Apps)
                .expect("test config should allow feature update");
        });

    let test = builder.build(&server).await?;
    let codex = Arc::clone(&test.forall);
    let cwd = Arc::clone(&test.cwd);
    let session_model = test.session_configured.model.clone();
    let cwd_path = cwd.abs();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd_path.as_path());

    assert_eq!(spawn_models_mock.requests().len(), 1);
    assert_eq!(spawn_models_mock.single_request_path(), "/v1/models");

    // 2) If the server sends a different X-Models-Etag on /responses, Forall refreshes /models.
    let refresh_models_mock = responses::mount_models_once_with_etag(
        &server,
        ModelsResponse { models: Vec::new() },
        ETAG_2,
    )
    .await;

    // First /responses request (user message) succeeds and returns a tool call.
    // It also includes a mismatched X-Models-Etag, which should trigger a /models refresh.
    let first_response_body = sse(vec![
        ev_response_created("resp-1"),
        ev_shell_command_call(CALL_ID, "/bin/echo 'etag ok'"),
        ev_completed("resp-1"),
    ]);
    responses::mount_response_once(
        &server,
        sse_response(first_response_body).insert_header("X-Models-Etag", ETAG_2),
    )
    .await;

    // Second /responses request (tool output) includes the same X-Models-Etag; Forall should not
    // refetch /models again after it has already refreshed the catalog.
    let completion_response_body = sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-1", "done"),
        ev_completed("resp-2"),
    ]);
    let tool_output_mock = responses::mount_response_once(
        &server,
        sse_response(completion_response_body).insert_header("X-Models-Etag", ETAG_2),
    )
    .await;

    codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "please run a tool".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: forall_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(cwd_path)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(forall_protocol::config_types::CollaborationMode {
                    mode: forall_protocol::config_types::ModeKind::Default,
                    settings: forall_protocol::config_types::Settings {
                        model: session_model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let _ = wait_for_event_with_timeout(
        &codex,
        |ev| matches!(ev, EventMsg::TurnComplete(_)),
        Duration::from_secs(30),
    )
    .await;

    // Assert /models was refreshed exactly once after the X-Models-Etag mismatch.
    assert_eq!(refresh_models_mock.requests().len(), 1);
    assert_eq!(refresh_models_mock.single_request_path(), "/v1/models");
    let refresh_req = refresh_models_mock
        .requests()
        .into_iter()
        .next()
        .expect("one request");
    // Ensure Forall includes client_version on refresh. (This is a stable signal that we're using the /models client.)
    assert!(
        refresh_req
            .url
            .query_pairs()
            .any(|(k, _)| k == "client_version"),
        "expected /models refresh to include client_version query param"
    );

    // Assert the tool output /responses request succeeded and did not trigger another /models fetch.
    let tool_req = tool_output_mock.single_request();
    let _ = tool_req.function_call_output(CALL_ID);
    assert_eq!(refresh_models_mock.requests().len(), 1);

    Ok(())
}
