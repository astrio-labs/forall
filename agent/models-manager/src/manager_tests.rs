use super::*;
use crate::ModelsManagerConfig;
use chrono::Utc;
use forall_login::AuthManager;
use forall_login::ForallAuth;
use forall_protocol::openai_models::ModelsResponse;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tempfile::tempdir;

#[path = "model_info_overrides_tests.rs"]
mod model_info_overrides_tests;

fn remote_model(slug: &str, display: &str, priority: i32) -> ModelInfo {
    remote_model_with_visibility(slug, display, priority, "list")
}

fn remote_model_with_visibility(
    slug: &str,
    display: &str,
    priority: i32,
    visibility: &str,
) -> ModelInfo {
    serde_json::from_value(json!({
            "slug": slug,
            "display_name": display,
            "description": format!("{display} desc"),
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "low", "description": "low"}, {"effort": "medium", "description": "medium"}],
            "shell_type": "shell_command",
            "visibility": visibility,
            "minimal_client_version": [0, 1, 0],
            "supported_in_api": true,
            "priority": priority,
            "upgrade": null,
            "base_instructions": "base instructions",
            "supports_reasoning_summaries": false,
            "support_verbosity": false,
            "default_verbosity": null,
            "apply_patch_tool_type": null,
            "truncation_policy": {"mode": "bytes", "limit": 10_000},
            "supports_parallel_tool_calls": false,
            "supports_image_detail_original": false,
            "context_window": 272_000,
            "max_context_window": 272_000,
            "experimental_supported_tools": [],
        }))
        .expect("valid model")
}

fn assert_models_contain(actual: &[ModelInfo], expected: &[ModelInfo]) {
    for model in expected {
        assert!(
            actual.iter().any(|candidate| candidate.slug == model.slug),
            "expected model {} in cached list",
            model.slug
        );
    }
}

#[derive(Debug)]
struct TestModelsEndpoint {
    has_command_auth: bool,
    uses_forall_backend: bool,
    uses_provider_model_catalog: bool,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
}

impl TestModelsEndpoint {
    fn new(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            has_command_auth: false,
            uses_forall_backend: true,
            uses_provider_model_catalog: false,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn provider_catalog(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            has_command_auth: false,
            uses_forall_backend: false,
            uses_provider_model_catalog: true,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn without_refresh(responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            has_command_auth: false,
            uses_forall_backend: false,
            uses_provider_model_catalog: false,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }

    async fn list_models(&self) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

impl ModelsEndpointClient for TestModelsEndpoint {
    fn has_command_auth(&self) -> bool {
        self.has_command_auth
    }

    fn uses_provider_model_catalog(&self) -> bool {
        self.uses_provider_model_catalog
    }

    fn uses_forall_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(async { self.uses_forall_backend })
    }

    fn list_models<'a>(
        &'a self,
        _client_version: &'a str,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(TestModelsEndpoint::list_models(self))
    }
}

fn openai_manager_for_tests(
    forall_home: std::path::PathBuf,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
) -> OpenAiModelsManager {
    openai_manager_for_tests_with_auth(
        forall_home,
        endpoint_client,
        Some(AuthManager::from_auth_for_testing(
            ForallAuth::create_dummy_api_key_auth_for_testing(),
        )),
    )
}

fn openai_manager_for_tests_with_auth(
    forall_home: std::path::PathBuf,
    endpoint_client: Arc<dyn ModelsEndpointClient>,
    auth_manager: Option<Arc<AuthManager>>,
) -> OpenAiModelsManager {
    OpenAiModelsManager::new(forall_home, endpoint_client, auth_manager)
}

fn static_manager_for_tests(model_catalog: ModelsResponse) -> StaticModelsManager {
    StaticModelsManager::new(/*auth_manager*/ None, model_catalog)
}

#[tokio::test]
async fn get_model_info_tracks_fallback_usage() {
    let forall_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        forall_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();

    let known = manager.get_model_info(known_slug.as_str(), &config).await;
    assert!(!known.used_fallback_model_metadata);
    assert_eq!(known.slug, known_slug);

    let unknown = manager
        .get_model_info("model-that-does-not-exist", &config)
        .await;
    assert!(unknown.used_fallback_model_metadata);
    assert_eq!(unknown.slug, "model-that-does-not-exist");
}

#[tokio::test]
async fn get_model_info_uses_custom_catalog() {
    let config = ModelsManagerConfig::default();
    let mut overlay = remote_model("gpt-overlay", "Overlay", /*priority*/ 0);
    overlay.supports_image_detail_original = true;

    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![overlay],
    });

    let model_info = manager
        .get_model_info("gpt-overlay-experiment", &config)
        .await;

    assert_eq!(model_info.slug, "gpt-overlay-experiment");
    assert_eq!(model_info.display_name, "Overlay");
    assert_eq!(model_info.context_window, Some(272_000));
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.supports_parallel_tool_calls);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_namespaced_suffix() {
    let config = ModelsManagerConfig::default();
    let mut remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    remote.supports_image_detail_original = true;
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "custom/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.supports_image_detail_original);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_matches_hyphenated_provider_namespace_suffix() {
    let config = ModelsManagerConfig::default();
    let remote = remote_model("gpt-image", "Image", /*priority*/ 0);
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![remote],
    });
    let namespaced_model = "openai-codex/gpt-image".to_string();

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_rejects_multi_segment_namespace_suffix_matching() {
    let forall_home = tempdir().expect("temp dir");
    let config = ModelsManagerConfig::default();
    let manager = openai_manager_for_tests(
        forall_home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let known_slug = manager
        .get_remote_models()
        .await
        .first()
        .expect("bundled models should include at least one model")
        .slug
        .clone();
    let namespaced_model = format!("ns1/ns2/{known_slug}");

    let model_info = manager.get_model_info(&namespaced_model, &config).await;

    assert_eq!(model_info.slug, namespaced_model);
    assert!(model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_sorts_by_priority() {
    let remote_models = vec![
        remote_model("priority-low", "Low", /*priority*/ 1),
        remote_model("priority-high", "High", /*priority*/ 0),
    ];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");
    let cached_remote = manager.get_remote_models().await;
    assert_models_contain(&cached_remote, &remote_models);

    let available = manager.list_models(RefreshStrategy::OnlineIfUncached).await;
    let high_idx = available
        .iter()
        .position(|model| model.model == "priority-high")
        .expect("priority-high should be listed");
    let low_idx = available
        .iter()
        .position(|model| model.model == "priority-low")
        .expect("priority-low should be listed");
    assert!(
        high_idx < low_idx,
        "higher priority should be listed before lower priority"
    );
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_merges_remote_with_bundled_for_forall_backend() {
    let remote_models = vec![remote_model(
        "forall-backend-visible",
        "Forall Backend Visible",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_reads_cached_catalog_across_manager_instances() {
    let remote_models = vec![remote_model(
        "cached-across-instances",
        "Cached Across Instances",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let fetch_endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let fetch_manager =
        openai_manager_for_tests(forall_home.path().to_path_buf(), fetch_endpoint.clone());

    fetch_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    let cache_endpoint = TestModelsEndpoint::new(Vec::new());
    let cache_manager =
        openai_manager_for_tests(forall_home.path().to_path_buf(), cache_endpoint.clone());

    cache_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");

    assert_models_contain(&cache_manager.get_remote_models().await, &remote_models);
    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );
}

#[tokio::test]
async fn refresh_available_models_uses_remote_only_catalog_for_provider_catalog() {
    let remote_models = vec![remote_model(
        "provider-visible-source-of-truth",
        "Provider Visible",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::provider_catalog(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, remote_models);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_cached_remote_only_catalog_for_provider_catalog() {
    let remote_models = vec![remote_model(
        "provider-cached-source-of-truth",
        "Provider Cached",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let fetch_endpoint = TestModelsEndpoint::provider_catalog(vec![remote_models.clone()]);
    let fetch_manager =
        openai_manager_for_tests(forall_home.path().to_path_buf(), fetch_endpoint.clone());

    fetch_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    let cache_endpoint = TestModelsEndpoint::provider_catalog(Vec::new());
    let cache_manager =
        openai_manager_for_tests(forall_home.path().to_path_buf(), cache_endpoint.clone());

    cache_manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");

    assert_eq!(cache_manager.get_remote_models().await, remote_models);
    assert_eq!(
        cache_endpoint.fetch_count(),
        0,
        "fresh cache should avoid a model fetch"
    );
}

#[tokio::test]
async fn get_model_info_uses_fallback_for_bundled_models_when_provider_remote_is_authoritative() {
    let remote_models = vec![remote_model(
        "provider-authoritative-model-info",
        "Provider Model Info",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::provider_catalog(vec![remote_models]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint);
    let bundled_slug = load_remote_models_from_file()
        .expect("bundled models should parse")
        .first()
        .expect("bundled models should contain at least one model")
        .slug
        .clone();

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let model_info = manager
        .get_model_info(&bundled_slug, &ModelsManagerConfig::default())
        .await;

    assert_eq!(model_info.slug, bundled_slug);
    assert!(model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn get_model_info_keeps_bundled_metadata_after_remote_refresh() {
    let remote_models = vec![remote_model(
        "remote-refresh-model-info",
        "Remote Refresh Model Info",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint);
    let bundled_slug = load_remote_models_from_file()
        .expect("bundled models should parse")
        .first()
        .expect("bundled models should contain at least one model")
        .slug
        .clone();

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    let model_info = manager
        .get_model_info(&bundled_slug, &ModelsManagerConfig::default())
        .await;

    assert_eq!(model_info.slug, bundled_slug);
    assert!(!model_info.used_fallback_model_metadata);
}

#[tokio::test]
async fn refresh_available_models_preserves_bundled_catalog_for_empty_chatgpt_remote() {
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![Vec::new()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint);
    let expected = load_remote_models_from_file().expect("bundled models should parse");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

#[tokio::test]
async fn refresh_available_models_merges_hidden_only_chatgpt_remote_with_bundled_catalog() {
    let hidden_remote = remote_model_with_visibility(
        "chatgpt-hidden-only",
        "ChatGPT Hidden",
        /*priority*/ 0,
        "hide",
    );
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![vec![hidden_remote.clone()]]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint);
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.push(hidden_remote);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
}

/// End-to-end guard for a BYOK OpenAI key: the raw `/v1/models` catalog is
/// merged with the bundled one, ranked, and turned into picker presets. The
/// first picker-visible preset is what the session defaults to, so this is the
/// path that used to hand the user `text-embedding-ada-002`.
#[tokio::test]
async fn api_key_catalog_defaults_to_the_most_capable_chat_model() {
    let body = json!({
        "object": "list",
        "data": [
            {"id": "text-embedding-ada-002"},
            {"id": "whisper-1"},
            {"id": "gpt-4o"},
            {"id": "gpt-5"},
            {"id": "gpt-5.2"},
            {"id": "gpt-5.4-mini"},
            {"id": "gpt-5.5"},
            {"id": "gpt-5.6-luna"},
            {"id": "gpt-5.6-terra"},
            {"id": "gpt-5.6-sol"},
        ]
    })
    .to_string();
    let remote_models = crate::openai_compatible_catalog::parse_model_catalog_response(
        body.as_bytes(),
    )
    .expect("catalog should parse");

    let forall_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        has_command_auth: true,
        uses_forall_backend: false,
        uses_provider_model_catalog: false,
        responses: Mutex::new(vec![remote_models].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let manager = openai_manager_for_tests_with_auth(
        forall_home.path().to_path_buf(),
        endpoint,
        Some(AuthManager::from_auth_for_testing(
            ForallAuth::from_api_key("test-api-key"),
        )),
    );

    let presets = manager.list_models(RefreshStrategy::OnlineIfUncached).await;
    let visible: Vec<&str> = presets
        .iter()
        .filter(|preset| preset.show_in_picker)
        .map(|preset| preset.model.as_str())
        .collect();

    assert_eq!(
        visible.first(),
        Some(&"gpt-5.6-sol"),
        "the flagship model must lead the picker, got {visible:?}"
    );
    assert!(
        !visible.iter().any(|slug| slug.contains("embedding")
            || *slug == "whisper-1"),
        "non-chat models must not reach the picker, got {visible:?}"
    );
    let default = presets
        .iter()
        .find(|preset| preset.is_default)
        .expect("a default preset should be marked");
    assert_eq!(default.model, "gpt-5.6-sol");
}

#[tokio::test]
async fn refresh_available_models_keeps_merging_for_api_auth() {
    let remote_models = vec![remote_model(
        "api-auth-visible-remote",
        "API Auth Visible",
        /*priority*/ 0,
    )];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = Arc::new(TestModelsEndpoint {
        has_command_auth: true,
        uses_forall_backend: false,
        uses_provider_model_catalog: false,
        responses: Mutex::new(vec![remote_models.clone()].into()),
        fetch_count: AtomicUsize::new(0),
    });
    let manager = openai_manager_for_tests_with_auth(
        forall_home.path().to_path_buf(),
        endpoint.clone(),
        Some(AuthManager::from_auth_for_testing(
            ForallAuth::from_api_key("test-api-key"),
        )),
    );
    let mut expected = load_remote_models_from_file().expect("bundled models should parse");
    expected.extend(remote_models);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, expected);
    assert_eq!(endpoint.fetch_count(), 1, "expected a single model fetch");
}

#[tokio::test]
async fn refresh_available_models_uses_cache_when_fresh() {
    let remote_models = vec![remote_model("cached", "Cached", /*priority*/ 5)];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::new(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("first refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);

    // Second call should read from cache and avoid the network.
    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("cached refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &remote_models);
    assert_eq!(
        endpoint.fetch_count(),
        1,
        "cache hit should avoid a second model fetch"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_cache_stale() {
    let initial_models = vec![remote_model("stale", "Stale", /*priority*/ 1)];
    let forall_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("fresh", "Fresh", /*priority*/ 9)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    // Rewrite cache with an old timestamp so it is treated as stale.
    manager
        .cache_manager
        .manipulate_cache_for_test(|fetched_at| {
            *fetched_at = Utc::now() - chrono::Duration::hours(1);
        })
        .await
        .expect("cache manipulation succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "stale cache refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_refetches_when_version_mismatch() {
    let initial_models = vec![remote_model("old", "Old", /*priority*/ 1)];
    let forall_home = tempdir().expect("temp dir");
    let updated_models = vec![remote_model("new", "New", /*priority*/ 2)];
    let endpoint = TestModelsEndpoint::new(vec![initial_models.clone(), updated_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    manager
        .cache_manager
        .mutate_cache_for_test(|cache| {
            let client_version = crate::client_version_to_whole();
            cache.client_version = Some(format!("{client_version}-mismatch"));
        })
        .await
        .expect("cache mutation succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");
    assert_models_contain(&manager.get_remote_models().await, &updated_models);
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "version mismatch should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_drops_removed_remote_models() {
    let initial_models = vec![remote_model(
        "remote-old",
        "Remote Old",
        /*priority*/ 1,
    )];
    let forall_home = tempdir().expect("temp dir");
    let refreshed_models = vec![remote_model(
        "remote-new",
        "Remote New",
        /*priority*/ 1,
    )];
    let endpoint = TestModelsEndpoint::new(vec![initial_models, refreshed_models]);
    let mut manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());
    manager.cache_manager.set_ttl(Duration::ZERO);

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("initial refresh succeeds");

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("second refresh succeeds");

    let available = manager
        .try_list_models()
        .expect("models should be available");
    assert!(
        available.iter().any(|preset| preset.model == "remote-new"),
        "new remote model should be listed"
    );
    assert!(
        !available.iter().any(|preset| preset.model == "remote-old"),
        "removed remote model should not be listed"
    );
    assert_eq!(
        endpoint.fetch_count(),
        2,
        "second refresh should fetch models again"
    );
}

#[tokio::test]
async fn refresh_available_models_skips_network_without_chatgpt_auth() {
    let dynamic_slug = "dynamic-model-only-for-test-noauth";
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::without_refresh(vec![vec![remote_model(
        dynamic_slug,
        "No Auth",
        /*priority*/ 1,
    )]]);
    let manager = openai_manager_for_tests_with_auth(
        forall_home.path().to_path_buf(),
        endpoint.clone(),
        /*auth_manager*/ None,
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should no-op without chatgpt auth");
    let cached_remote = manager.get_remote_models().await;
    assert!(
        !cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should be skipped without chatgpt auth"
    );
    assert_eq!(
        endpoint.fetch_count(),
        0,
        "endpoint that cannot refresh should avoid model fetches"
    );
}

#[derive(Debug)]
struct TestAuthAwareModelsEndpoint {
    auth_manager: Option<Arc<AuthManager>>,
    responses: Mutex<VecDeque<Vec<ModelInfo>>>,
    fetch_count: AtomicUsize,
}

impl TestAuthAwareModelsEndpoint {
    fn new(auth_manager: Option<Arc<AuthManager>>, responses: Vec<Vec<ModelInfo>>) -> Arc<Self> {
        Arc::new(Self {
            auth_manager,
            responses: Mutex::new(responses.into()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn fetch_count(&self) -> usize {
        self.fetch_count.load(Ordering::SeqCst)
    }

    async fn uses_forall_backend(&self) -> bool {
        match self.auth_manager.as_ref() {
            Some(auth_manager) => auth_manager
                .auth()
                .await
                .as_ref()
                .is_some_and(ForallAuth::uses_forall_backend),
            None => false,
        }
    }

    async fn list_models(&self) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
        let models = self
            .responses
            .lock()
            .expect("responses lock should not be poisoned")
            .pop_front()
            .unwrap_or_default();
        Ok((models, None))
    }
}

impl ModelsEndpointClient for TestAuthAwareModelsEndpoint {
    fn has_command_auth(&self) -> bool {
        false
    }

    fn uses_provider_model_catalog(&self) -> bool {
        false
    }

    fn uses_forall_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(TestAuthAwareModelsEndpoint::uses_forall_backend(self))
    }

    fn list_models<'a>(
        &'a self,
        _client_version: &'a str,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(TestAuthAwareModelsEndpoint::list_models(self))
    }
}

#[tokio::test]
async fn refresh_available_models_skips_network_with_api_key_auth() {
    let dynamic_slug = "dynamic-model-only-for-test-api-key";
    let forall_home = tempdir().expect("temp dir");
    let auth_manager =
        AuthManager::from_auth_for_testing(ForallAuth::create_dummy_api_key_auth_for_testing());
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(Arc::clone(&auth_manager)),
        vec![vec![remote_model(
            dynamic_slug,
            "API Key",
            /*priority*/ 1,
        )]],
    );
    let manager = openai_manager_for_tests_with_auth(
        forall_home.path().to_path_buf(),
        endpoint.clone(),
        Some(auth_manager),
    );

    manager
        .refresh_available_models(RefreshStrategy::Online)
        .await
        .expect("refresh should no-op with API key auth");
    let cached_remote = manager.get_remote_models().await;

    assert!(
        !cached_remote
            .iter()
            .any(|candidate| candidate.slug == dynamic_slug),
        "remote refresh should be skipped when API key auth is active"
    );
    assert_eq!(
        endpoint.fetch_count(),
        0,
        "endpoint should avoid model fetches when API key auth is active"
    );
}

#[test]
fn build_available_models_picks_default_after_hiding_hidden_models() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });

    let hidden_model =
        remote_model_with_visibility("hidden", "Hidden", /*priority*/ 0, "hide");
    let visible_model =
        remote_model_with_visibility("visible", "Visible", /*priority*/ 1, "list");

    let expected_hidden = ModelPreset::from(hidden_model.clone());
    let mut expected_visible = ModelPreset::from(visible_model.clone());
    expected_visible.is_default = true;

    let available = manager.build_available_models(vec![hidden_model, visible_model]);

    assert_eq!(available, vec![expected_hidden, expected_visible]);
}

#[tokio::test]
async fn static_manager_filters_models_unsupported_in_api() {
    let auth_manager =
        AuthManager::from_auth_for_testing(ForallAuth::create_dummy_api_key_auth_for_testing());
    let chatgpt_only_model = {
        let mut model = remote_model("chatgpt-only", "ChatGPT Only", /*priority*/ 0);
        model.supported_in_api = false;
        model
    };
    let api_model = remote_model("api-model", "API Model", /*priority*/ 1);
    let manager = StaticModelsManager::new(
        Some(auth_manager),
        ModelsResponse {
            models: vec![chatgpt_only_model, api_model],
        },
    );

    let api_models = manager.list_models(RefreshStrategy::Online).await;

    assert_eq!(
        api_models
            .iter()
            .map(|model| model.model.as_str())
            .collect::<Vec<_>>(),
        vec!["api-model"]
    );
}

#[test]
fn bundled_models_json_roundtrips() {
    let response = crate::bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));

    let serialized =
        serde_json::to_string(&response).expect("bundled models.json should serialize");
    let roundtripped: ModelsResponse =
        serde_json::from_str(&serialized).expect("serialized models.json should deserialize");

    assert_eq!(
        response, roundtripped,
        "bundled models.json should round trip through serde"
    );
    assert!(
        !response.models.is_empty(),
        "bundled models.json should contain at least one model"
    );
}

#[test]
fn bundled_models_use_canonical_instructions() {
    let response = crate::bundled_models_response()
        .unwrap_or_else(|err| panic!("bundled models.json should parse: {err}"));
    let expected = crate::model_info::with_config_overrides(
        crate::model_info::model_info_from_slug("unknown-model"),
        &crate::ModelsManagerConfig::default(),
    )
    .base_instructions;

    for model in response.models {
        assert_eq!(
            model.base_instructions, expected,
            "model {} should use canonical prompt.md instructions",
            model.slug
        );
        assert!(
            model.model_messages.is_none(),
            "model {} should not ship per-model personality templates",
            model.slug
        );
    }
}

#[tokio::test]
async fn refresh_available_models_uses_provider_catalog_without_bundled_merge() {
    let remote_models = vec![
        remote_model("anthropic/claude-sonnet-4", "Claude Sonnet 4", 0),
        remote_model("google/gemini-2.5-pro", "Gemini 2.5 Pro", 1),
    ];
    let forall_home = tempdir().expect("temp dir");
    let endpoint = TestModelsEndpoint::provider_catalog(vec![remote_models.clone()]);
    let manager = openai_manager_for_tests(forall_home.path().to_path_buf(), endpoint.clone());

    assert!(
        manager.get_remote_models().await.is_empty(),
        "provider catalog should not seed bundled GPT models"
    );

    manager
        .refresh_available_models(RefreshStrategy::OnlineIfUncached)
        .await
        .expect("refresh succeeds");

    assert_eq!(manager.get_remote_models().await, remote_models);
    assert_eq!(endpoint.fetch_count(), 1);
}
