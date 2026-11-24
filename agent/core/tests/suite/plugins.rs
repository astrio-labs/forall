#![cfg(not(target_os = "windows"))]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_forall::TestForall;
use core_test_support::test_forall::test_forall;
use core_test_support::wait_for_event;
use forall_analytics::DEFAULT_ANALYTICS_EVENTS_PATH;
use forall_login::ForallAuth;
use forall_protocol::protocol::EventMsg;
use forall_protocol::protocol::Op;
use tempfile::TempDir;
use wiremock::MockServer;

const SAMPLE_PLUGIN_CONFIG_NAME: &str = "sample@test";
const SAMPLE_PLUGIN_DISPLAY_NAME: &str = "sample";
const SAMPLE_PLUGIN_DESCRIPTION: &str = "inspect sample data";

fn sample_plugin_root(home: &TempDir) -> std::path::PathBuf {
    home.path().join("plugins/cache/test/sample/local")
}

fn write_sample_plugin_manifest_and_config(home: &TempDir) -> std::path::PathBuf {
    let plugin_root = sample_plugin_root(home);
    std::fs::create_dir_all(plugin_root.join(".forall-plugin"))
        .expect("create plugin manifest dir");
    std::fs::write(
        plugin_root.join(".forall-plugin/plugin.json"),
        format!(
            r#"{{"name":"{SAMPLE_PLUGIN_DISPLAY_NAME}","description":"{SAMPLE_PLUGIN_DESCRIPTION}"}}"#
        ),
    )
    .expect("write plugin manifest");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[features]\nplugins = true\n\n[plugins.\"{SAMPLE_PLUGIN_CONFIG_NAME}\"]\nenabled = true\n"
        ),
    )
    .expect("write config");
    plugin_root
}

fn write_plugin_skill_plugin(home: &TempDir) -> std::path::PathBuf {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    let skill_dir = plugin_root.join("skills/sample-search");
    std::fs::create_dir_all(skill_dir.as_path()).expect("create plugin skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: inspect sample data\n---\n\n# body\n",
    )
    .expect("write plugin skill");
    skill_dir.join("SKILL.md")
}

async fn build_analytics_plugin_test_forall(
    server: &MockServer,
    forall_home: Arc<TempDir>,
) -> Result<TestForall> {
    let analytics_endpoint = format!(
        "{}{}",
        server.uri().trim_end_matches('/'),
        DEFAULT_ANALYTICS_EVENTS_PATH
    );
    let mut builder = test_forall()
        .with_home(forall_home)
        .with_auth(ForallAuth::create_dummy_api_key_auth_for_testing())
        .with_model("gpt-5.2")
        .with_config(move |config| {
            config.analytics_endpoint = Some(analytics_endpoint);
            config.analytics_enabled = Some(true);
        });
    Ok(builder
        .build(server)
        .await
        .expect("create new conversation"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Forall/ChatGPT backend auth stubbed out in BYOK build"]
async fn explicit_plugin_mentions_track_plugin_used_analytics() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let _resp_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let forall_home = Arc::new(TempDir::new()?);
    write_plugin_skill_plugin(forall_home.as_ref());
    let test_forall = build_analytics_plugin_test_forall(&server, forall_home).await?;
    let codex = Arc::clone(&test_forall.forall);

    codex
        .submit(Op::UserInput {
            items: vec![forall_protocol::user_input::UserInput::Mention {
                name: "sample".into(),
                path: format!("plugin://{SAMPLE_PLUGIN_CONFIG_NAME}"),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&codex, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let deadline = Instant::now() + Duration::from_secs(10);
    let plugin_event = loop {
        let requests = server.received_requests().await.unwrap_or_default();
        if let Some(event) = requests
            .into_iter()
            .filter(|request| request.url.path() == DEFAULT_ANALYTICS_EVENTS_PATH)
            .find_map(|request| {
                let payload: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
                payload["events"].as_array().and_then(|events| {
                    events
                        .iter()
                        .find(|event| event["event_type"] == "forall_plugin_used")
                        .cloned()
                })
            })
        {
            break event;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for plugin analytics request");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let event = plugin_event;
    assert_eq!(event["event_params"]["plugin_id"], "sample@test");
    assert_eq!(event["event_params"]["plugin_name"], "sample");
    assert_eq!(event["event_params"]["marketplace_name"], "test");
    assert_eq!(event["event_params"]["has_skills"], true);
    assert_eq!(event["event_params"]["mcp_server_count"], 0);
    assert_eq!(
        event["event_params"]["mcp_server_names"],
        serde_json::json!([])
    );
    assert_eq!(
        event["event_params"]["connector_ids"],
        serde_json::json!([])
    );
    assert_eq!(
        event["event_params"]["product_client_id"],
        serde_json::json!(forall_login::default_client::originator().value)
    );
    assert_eq!(event["event_params"]["model_slug"], "gpt-5.2");
    assert!(event["event_params"]["thread_id"].as_str().is_some());
    assert!(event["event_params"]["turn_id"].as_str().is_some());

    Ok(())
}
