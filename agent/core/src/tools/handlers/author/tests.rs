use super::*;
use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolCallSource;
use crate::turn_diff_tracker::TurnDiffTracker;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn invocation(handler: &AuthorHandler, arguments: Value) -> ToolInvocation {
    let (session, turn) = make_session_and_context().await;
    ToolInvocation {
        session: Arc::new(session),
        turn: Arc::new(turn),
        cancellation_token: tokio_util::sync::CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-author".to_string(),
        tool_name: handler.tool_name(),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

#[tokio::test]
async fn init_preview_is_non_mutating_and_apply_writes() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = CanonicalRoot::new(directory.path()).expect("canonical root");
    let handler = AuthorHandler::init();

    let preview_invocation = invocation(&handler, json!({ "mode": "preview" })).await;
    let preview = handler
        .execute_at_root(&root, &preview_invocation)
        .expect("preview output");
    assert_eq!(
        preview.code_mode_result(&preview_invocation.payload)["ok"],
        json!(true)
    );
    assert!(!directory.path().join(MAPPING_PATH).exists());

    let apply_invocation = invocation(&handler, json!({ "mode": "apply" })).await;
    let applied = handler
        .execute_at_root(&root, &apply_invocation)
        .expect("apply output");
    assert_eq!(
        applied.code_mode_result(&apply_invocation.payload)["ok"],
        json!(true)
    );
    assert!(directory.path().join(MAPPING_PATH).is_file());
}

#[tokio::test]
async fn discover_returns_structured_symbols() {
    let directory = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(directory.path().join("src")).expect("create src");
    std::fs::write(
        directory.path().join("src/clamp.ts"),
        "export function clamp(x: number) {\n  return x;\n}\n",
    )
    .expect("write source");
    let root = CanonicalRoot::new(directory.path()).expect("canonical root");
    let handler = AuthorHandler::discover();
    let invocation = invocation(
        &handler,
        json!({ "files": ["src/clamp.ts"], "languages": [] }),
    )
    .await;

    let output = handler
        .execute_at_root(&root, &invocation)
        .expect("discover output")
        .code_mode_result(&invocation.payload);
    assert_eq!(output["ok"], json!(true));
    assert_eq!(output["result"]["symbols"][0]["name"], json!("clamp"));
}

#[tokio::test]
async fn mutation_hook_payload_only_exists_for_apply() {
    let handler = AuthorHandler::scaffold_contracts();
    let preview = invocation(
        &handler,
        json!({
            "mode": "preview",
            "contracts": [{ "file": "src/clamp.ts", "symbol": "clamp" }]
        }),
    )
    .await;
    assert_eq!(handler.pre_tool_use_payload(&preview), None);

    let apply = invocation(
        &handler,
        json!({
            "mode": "apply",
            "contracts": [{ "file": "src/clamp.ts", "symbol": "clamp" }]
        }),
    )
    .await;
    assert_eq!(
        handler.pre_tool_use_payload(&apply),
        Some(PreToolUsePayload {
            tool_name: HookToolName::new("author.scaffold_contracts"),
            tool_input: json!({
                "mode": "apply",
                "contracts": [{ "file": "src/clamp.ts", "symbol": "clamp" }],
                "paths": ["src/clamp.ts"]
            }),
        })
    );
}

#[test]
fn only_read_only_author_tools_support_parallel_calls() {
    assert!(AuthorHandler::status().supports_parallel_tool_calls());
    assert!(AuthorHandler::discover().supports_parallel_tool_calls());
    assert!(AuthorHandler::validate().supports_parallel_tool_calls());
    assert!(!AuthorHandler::init().supports_parallel_tool_calls());
    assert!(!AuthorHandler::upsert_requirements().supports_parallel_tool_calls());
    assert!(!AuthorHandler::scaffold_contracts().supports_parallel_tool_calls());
    assert!(!AuthorHandler::scaffold_property().supports_parallel_tool_calls());
}

#[tokio::test]
async fn unsafe_paths_return_structured_authoring_errors() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = CanonicalRoot::new(directory.path()).expect("canonical root");
    let handler = AuthorHandler::discover();
    let invocation = invocation(&handler, json!({ "files": ["../outside.ts"] })).await;

    let output = handler
        .execute_at_root(&root, &invocation)
        .expect("structured error")
        .code_mode_result(&invocation.payload);
    assert_eq!(output["ok"], json!(false));
    assert_eq!(output["error"]["code"], json!("unsafe_path"));
    assert_eq!(output["error"]["path"], json!("../outside.ts"));
}
