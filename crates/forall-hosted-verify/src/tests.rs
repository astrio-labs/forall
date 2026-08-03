#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::sync::Arc;
use std::sync::Mutex;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::post;
use serde_json::Value;
use tempfile::TempDir;

use super::*;

#[derive(Clone, Default)]
struct MockState {
    calls: Arc<Mutex<Vec<String>>>,
    reject_status_with_secret: bool,
}

async fn mock_mcp(
    State(state): State<MockState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert_eq!(
        headers.get("authorization"),
        Some(&HeaderValue::from_static("Bearer secret-token"))
    );
    let method = body["method"].as_str().expect("method");
    state
        .calls
        .lock()
        .expect("calls lock")
        .push(method.to_string());
    if method == "initialize" {
        let mut response_headers = HeaderMap::new();
        response_headers.insert("Mcp-Session-Id", HeaderValue::from_static("session-1"));
        return (
            StatusCode::OK,
            response_headers,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": body["id"],
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "serverInfo": {"name": "mock", "version": "1"}
                }
            })),
        )
            .into_response();
    }
    assert_eq!(
        headers.get("mcp-session-id"),
        Some(&HeaderValue::from_static("session-1"))
    );
    if method == "notifications/initialized" {
        return StatusCode::ACCEPTED.into_response();
    }
    let name = body["params"]["name"].as_str().expect("tool name");
    state
        .calls
        .lock()
        .expect("calls lock")
        .push(name.to_string());
    if state.reject_status_with_secret && name == "forall_verification_status" {
        return Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": body["id"],
            "error": {"code": -32000, "message": "bad secret-token"}
        }))
        .into_response();
    }
    let payload = match name {
        "forall_verify" => serde_json::json!({
            "contract_version": 1,
            "job_id": "vrf_1",
            "status": "queued",
            "submitted_at": "2026-07-12T00:00:00Z",
            "poll_after_ms": 100
        }),
        "forall_verification_status" => serde_json::json!({
            "contract_version": 1,
            "job_id": "vrf_1",
            "status": "running",
            "submitted_at": "2026-07-12T00:00:00Z",
            "updated_at": "2026-07-12T00:00:01Z",
            "progress": {"phase": "proofs", "completed": 1, "total": 2}
        }),
        "forall_cancel_verification" => serde_json::json!({
            "contract_version": 1,
            "job_id": "vrf_1",
            "status": "cancelled",
            "submitted_at": "2026-07-12T00:00:00Z",
            "updated_at": "2026-07-12T00:00:01Z"
        }),
        "forall_explain_verification" => serde_json::json!({
            "contract_version": 1,
            "job_id": "vrf_1",
            "summary": "Fix the contract.",
            "actions": ["Strengthen the postcondition."]
        }),
        other => panic!("unexpected tool {other}"),
    };
    Json(serde_json::json!({
        "jsonrpc": "2.0",
        "id": body["id"],
        "result": {"structuredContent": payload}
    }))
    .into_response()
}

async fn start_mock(state: MockState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/mcp", post(mock_mcp))
                .with_state(state),
        )
        .await
        .expect("server");
    });
    format!("http://{address}/mcp")
}

fn client(endpoint: String) -> HostedVerificationClient {
    HostedVerificationClient::with_endpoint(
        endpoint,
        Arc::new(StaticBearerTokenProvider::new("secret-token").expect("token")),
    )
}

#[tokio::test]
async fn initializes_session_and_calls_all_hosted_tools() {
    let state = MockState::default();
    let endpoint = start_mock(state.clone()).await;
    let client = client(endpoint);
    let submitted = client
        .submit(&SubmitVerificationRequest {
            source: github_source("owner/repo", "main", None),
            scope: VerificationScope::Project,
            strict: true,
            phases: vec![VerificationPhase::Proofs],
            pbt_seed: None,
            pbt_examples: None,
            requirement_ids: None,
        })
        .await
        .expect("submit");
    assert_eq!(submitted.job_id, "vrf_1");
    assert_eq!(
        client.status("vrf_1").await.expect("status").status,
        VerificationStatus::Running
    );
    assert_eq!(
        client.cancel("vrf_1").await.expect("cancel").status,
        VerificationStatus::Cancelled
    );
    let explained = client
        .explain(&ExplainVerificationRequest {
            job_id: "vrf_1".to_string(),
            issue_indexes: vec![0],
            audience: ExplanationAudience::Concise,
        })
        .await
        .expect("explain");
    assert_eq!(explained.summary, "Fix the contract.");
    assert_eq!(
        state.calls.lock().expect("calls").as_slice(),
        [
            "initialize",
            "notifications/initialized",
            "tools/call",
            "forall_verify",
            "tools/call",
            "forall_verification_status",
            "tools/call",
            "forall_cancel_verification",
            "tools/call",
            "forall_explain_verification",
        ]
    );
}

#[tokio::test]
async fn bearer_token_is_redacted_from_debug_and_errors() {
    let state = MockState {
        reject_status_with_secret: true,
        ..MockState::default()
    };
    let client = client(start_mock(state).await);
    assert!(!format!("{client:?}").contains("secret-token"));
    assert!(
        !format!(
            "{:?}",
            StaticBearerTokenProvider::new("secret-token").expect("token")
        )
        .contains("secret-token")
    );
    let error = client.status("vrf_1").await.expect_err("RPC error");
    let rendered = format!("{error:?} {error}");
    assert!(!rendered.contains("secret-token"));
    assert!(rendered.contains("[REDACTED]"));
}

#[test]
fn snapshot_is_sorted_and_excludes_secrets_and_build_outputs() {
    let root = TempDir::new().expect("tempdir");
    write(root.path(), ".forall/verify/mapping.yaml", "version: 1\n");
    write(root.path(), "src/z.rs", "pub fn z() {}\n");
    write(root.path(), "src/a.rs", "pub fn a() {}\n");
    write(root.path(), "support/data.txt", "support\n");
    write(root.path(), ".env", "TOKEN=secret\n");
    write(root.path(), ".git/config", "secret\n");
    write(root.path(), "target/output.txt", "artifact\n");

    let VerificationSource::Inline { files } = SnapshotPacker::default()
        .pack_workspace(root.path())
        .expect("snapshot")
    else {
        panic!("expected inline source");
    };
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [
            ".forall/verify/mapping.yaml",
            "src/a.rs",
            "src/z.rs",
            "support/data.txt"
        ]
    );
}

#[test]
fn verification_snapshot_selects_project_truth_mapped_sources_and_manifests() {
    let root = TempDir::new().expect("tempdir");
    write(
        root.path(),
        ".forall/verify/mapping.yaml",
        "version: 1\nrequirements:\n  - id: R1\n    code:\n      file: src/lib.rs\n      symbols: [clamp]\n",
    );
    write(root.path(), ".forall/verify/cache/report.json", "{}");
    write(root.path(), "src/lib.rs", "pub fn clamp() {}\n");
    write(root.path(), "src/unmapped.rs", "pub fn unrelated() {}\n");
    write(root.path(), "Cargo.toml", "[package]\nname = \"demo\"\n");
    write(root.path(), ".env", "TOKEN=secret\n");

    let VerificationSource::Inline { files } = SnapshotPacker::default()
        .pack_verification_workspace(root.path())
        .expect("verification snapshot")
    else {
        panic!("expected inline source");
    };
    let paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        [".forall/verify/mapping.yaml", "Cargo.toml", "src/lib.rs"]
    );
}

#[test]
fn verification_snapshot_rejects_secret_paths_from_mapping() {
    let root = TempDir::new().expect("tempdir");
    write(
        root.path(),
        ".forall/verify/mapping.yaml",
        "version: 1\nrequirements:\n  - id: R1\n    code:\n      file: .env\n      symbols: [secret]\n",
    );
    write(root.path(), ".env", "TOKEN=secret\n");
    assert!(matches!(
        SnapshotPacker::default().pack_verification_workspace(root.path()),
        Err(SnapshotError::Excluded(path)) if path == std::path::Path::new(".env")
    ));
}

#[test]
fn snapshot_rejects_traversal_non_utf8_and_limits() {
    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside");
    write(outside.path(), "outside.txt", "no\n");
    assert!(matches!(
        SnapshotPacker::default().pack_paths(root.path(), ["../outside.txt"]),
        Err(SnapshotError::InvalidPath(_))
    ));

    fs::write(root.path().join("binary"), [0xff, 0xfe]).expect("binary");
    assert!(matches!(
        SnapshotPacker::default().pack_workspace(root.path()),
        Err(SnapshotError::NonUtf8(_))
    ));
    fs::remove_file(root.path().join("binary")).expect("remove binary");

    fs::write(
        root.path().join("large.txt"),
        vec![b'x'; MAX_FILE_BYTES as usize + 1],
    )
    .expect("large file");
    assert!(matches!(
        SnapshotPacker::default().pack_workspace(root.path()),
        Err(SnapshotError::FileTooLarge { .. })
    ));
    fs::remove_file(root.path().join("large.txt")).expect("remove large");

    for index in 0..=MAX_SNAPSHOT_FILES {
        write(root.path(), &format!("files/{index:03}.txt"), "x");
    }
    assert!(matches!(
        SnapshotPacker::default().pack_workspace(root.path()),
        Err(SnapshotError::TooManyFiles { .. })
    ));
}

#[test]
fn snapshot_enforces_total_limit() {
    let root = TempDir::new().expect("tempdir");
    for index in 0..11 {
        fs::write(
            root.path().join(format!("{index}.txt")),
            vec![b'x'; MAX_FILE_BYTES as usize],
        )
        .expect("file");
    }
    assert!(matches!(
        SnapshotPacker::default().pack_workspace(root.path()),
        Err(SnapshotError::SnapshotTooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn snapshot_rejects_symlinks_without_following_them() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside");
    write(outside.path(), "secret.txt", "secret");
    symlink(
        outside.path().join("secret.txt"),
        root.path().join("linked.txt"),
    )
    .expect("symlink");
    assert!(matches!(
        SnapshotPacker::default().pack_workspace(root.path()),
        Err(SnapshotError::Symlink(path)) if path == std::path::Path::new("linked.txt")
    ));
    assert!(matches!(
        SnapshotPacker::default().pack_paths(root.path(), ["linked.txt"]),
        Err(SnapshotError::Symlink(_))
    ));
}

#[tokio::test]
async fn a_plaintext_endpoint_is_refused_before_the_token_is_sent() {
    // Nothing is listening on this host; reaching the network at all would be
    // a transport error rather than the rejection asserted here.
    let refused = client("http://mcp.example.com/mcp".to_string())
        .initialize()
        .await;
    assert!(matches!(
        refused,
        Err(HostedVerifyError::InsecureEndpoint(scheme)) if scheme == "http"
    ));

    let accepted = client("https://mcp.example.com/mcp".to_string())
        .initialize()
        .await;
    assert!(!matches!(
        accepted,
        Err(HostedVerifyError::InsecureEndpoint(_))
    ));
}

#[cfg(unix)]
#[test]
fn verification_snapshot_rejects_symlinked_support_files() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside");
    write(outside.path(), "secret.txt", "secret");
    write(
        root.path(),
        ".forall/verify/mapping.yaml",
        "version: 1\nrequirements: []\n",
    );
    // A root manifest is picked up by name, so it must not be a way to read a
    // file from outside the workspace.
    symlink(
        outside.path().join("secret.txt"),
        root.path().join("Cargo.toml"),
    )
    .expect("symlink");

    assert!(matches!(
        SnapshotPacker::default().pack_verification_workspace(root.path()),
        Err(SnapshotError::Symlink(path)) if path == std::path::Path::new("Cargo.toml")
    ));
}

#[cfg(unix)]
#[test]
fn verification_snapshot_rejects_a_symlinked_forall_directory() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("tempdir");
    let outside = TempDir::new().expect("outside");
    write(
        outside.path(),
        "verify/mapping.yaml",
        "version: 1\nrequirements: []\n",
    );
    write(outside.path(), "private.txt", "secret");
    // `WalkDir` follows a symlinked walk root, so `.forall` itself has to be
    // rejected before the walk starts.
    symlink(outside.path(), root.path().join(".forall")).expect("symlink");

    assert!(matches!(
        SnapshotPacker::default().pack_verification_workspace(root.path()),
        Err(SnapshotError::Symlink(path)) if path == std::path::Path::new(".forall")
    ));
}

#[test]
fn github_helper_has_explicit_wire_variant() {
    let source = github_source("owner/repository", "main", Some("packages/app".to_string()));
    assert_eq!(
        serde_json::to_value(source).expect("serialize"),
        serde_json::json!({
            "type": "github",
            "repository": "owner/repository",
            "ref": "main",
            "subdirectory": "packages/app"
        })
    );
}

fn write(root: &std::path::Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}
