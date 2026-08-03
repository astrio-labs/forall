#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::post;
use forall_authoring::authoring;
use forall_authoring::authoring::CanonicalRoot;
use forall_authoring::authoring::ContractTarget;
use forall_authoring::authoring::MutationMode;
use forall_authoring::authoring::MutationOptions;
use forall_authoring::authoring::ScaffoldContractsRequest;
use forall_authoring::authoring::UpsertRequirementsRequest;
use forall_authoring::mapping::schema::CodeRef;
use forall_authoring::mapping::schema::Requirement;
use forall_hosted_verify::HostedVerificationClient;
use forall_hosted_verify::SnapshotPacker;
use forall_hosted_verify::StaticBearerTokenProvider;
use forall_hosted_verify::SubmitVerificationRequest;
use forall_hosted_verify::VerificationScope;
use forall_hosted_verify::VerificationSource;
use serde_json::Value;

async fn hosted_mcp(headers: HeaderMap, Json(body): Json<Value>) -> Response {
    assert_eq!(
        headers.get("authorization"),
        Some(&HeaderValue::from_static("Bearer forall_test"))
    );
    match body["method"].as_str().expect("method") {
        "initialize" => {
            let mut response_headers = HeaderMap::new();
            response_headers.insert("Mcp-Session-Id", HeaderValue::from_static("standalone-e2e"));
            (
                StatusCode::OK,
                response_headers,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                    "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "serverInfo": {"name": "mock-forall", "version": "1"}
                    }
                })),
            )
                .into_response()
        }
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/call" => {
            assert_eq!(body["params"]["name"], "forall_verify");
            let files = body["params"]["arguments"]["source"]["files"]
                .as_array()
                .expect("inline files");
            assert!(files.iter().any(|file| {
                file["path"] == "src/clamp.ts"
                    && file["content"]
                        .as_str()
                        .is_some_and(|content| content.contains("//@ ensures result >= 0"))
            }));
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": body["id"],
                "result": {
                    "structuredContent": {
                        "contract_version": 1,
                        "job_id": "vrf_standalone",
                        "status": "queued",
                        "submitted_at": "2026-07-12T00:00:00Z",
                        "poll_after_ms": 100
                    }
                }
            }))
            .into_response()
        }
        other => panic!("unexpected method {other}"),
    }
}

#[tokio::test]
async fn authors_validates_packages_and_submits_without_a_local_mcp() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join("src")).expect("source directory");
    std::fs::write(
        workspace.path().join("src/clamp.ts"),
        "export function clamp(value: number) {\n  return value < 0 ? 0 : value;\n}\n",
    )
    .expect("source");
    let root = CanonicalRoot::new(workspace.path()).expect("canonical root");
    let apply = MutationOptions {
        mode: MutationMode::Apply,
        expected_sha256: BTreeMap::new(),
    };
    authoring::init_project(&root, &apply).expect("initialize authoring");

    let mapping_sha = authoring::project_status(&root)
        .expect("status")
        .mapping_sha256
        .expect("mapping sha");
    authoring::upsert_requirements(
        &root,
        &UpsertRequirementsRequest {
            requirements: vec![Requirement {
                id: "REQ-CLAMP".to_string(),
                capability: "bounds".to_string(),
                requirement: "The result is never negative.".to_string(),
                verified: true,
                property_tested: false,
                property: None,
                code: Some(CodeRef {
                    file: "src/clamp.ts".to_string(),
                    symbols: vec!["clamp".to_string()],
                }),
                contract: Some("result >= 0".to_string()),
                claimcheck: None,
                scenarios: None,
            }],
            mutation: MutationOptions {
                mode: MutationMode::Apply,
                expected_sha256: BTreeMap::from([(
                    ".forall/verify/mapping.yaml".to_string(),
                    mapping_sha,
                )]),
            },
        },
    )
    .expect("map requirement");

    let discovery =
        authoring::discover_symbols(&root, &["src/clamp.ts".to_string()]).expect("discover");
    let source_sha = discovery.files[0].sha256.clone();
    authoring::scaffold_contracts(
        &root,
        &ScaffoldContractsRequest {
            contracts: vec![ContractTarget {
                file: "src/clamp.ts".to_string(),
                symbol: "clamp".to_string(),
                requires: Vec::new(),
                ensures: vec!["result >= 0".to_string()],
            }],
            mutation: MutationOptions {
                mode: MutationMode::Apply,
                expected_sha256: BTreeMap::from([("src/clamp.ts".to_string(), source_sha)]),
            },
        },
    )
    .expect("scaffold contract");
    let validation = authoring::validate_authoring(&root).expect("validate authoring");
    assert!(validation.valid, "{:?}", validation.issues);

    let source = SnapshotPacker::default()
        .pack_verification_workspace(workspace.path())
        .expect("package verification snapshot");
    let VerificationSource::Inline { ref files } = source else {
        panic!("expected inline source");
    };
    assert!(files.iter().any(|file| file.path == "src/clamp.ts"));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/mcp", post(hosted_mcp)))
            .await
            .expect("mock server");
    });
    let client = HostedVerificationClient::with_endpoint(
        format!("http://{address}/mcp"),
        Arc::new(StaticBearerTokenProvider::new("forall_test").expect("token")),
    );
    let accepted = client
        .submit(&SubmitVerificationRequest {
            source,
            scope: VerificationScope::Project,
            strict: true,
            phases: Vec::new(),
            pbt_seed: Some(42),
            pbt_examples: Some(100),
            requirement_ids: None,
        })
        .await
        .expect("hosted submit");
    assert_eq!(accepted.job_id, "vrf_standalone");
}
