use forall_model_provider_info::OPENROUTER_DEFAULT_MODEL;
use forall_protocol::openai_models::ModelInfo;
use forall_protocol::openai_models::ModelVisibility;
use forall_protocol::openai_models::ModelsResponse;
use forall_protocol::openai_models::ReasoningEffort;
use forall_protocol::openai_models::ReasoningEffortPreset;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::model_info;
use crate::openai_model_ranking;

/// Parse a `/models` response in either Codex `ModelsResponse` or OpenAI-compatible list format.
pub fn parse_model_catalog_response(body: &[u8]) -> Result<Vec<ModelInfo>, serde_json::Error> {
    if let Ok(response) = serde_json::from_slice::<ModelsResponse>(body) {
        return Ok(response.models);
    }

    let list = serde_json::from_slice::<OpenAiModelsListResponse>(body)?;
    Ok(models_from_openai_compatible_list(list.data))
}

/// Filter a catalog to conversational models and order it most-capable-first.
///
/// Classification is per entry, not per catalog: a proxy is free to serve flat
/// OpenAI ids and namespaced vendor ids side by side, and judging the whole
/// response by one namespaced id would wave the first-party half through
/// unfiltered — putting an embedding model back at the top of the picker.
/// Namespaced entries are exempt from both filtering and scoring, so a
/// wholly-namespaced catalog keeps upstream order exactly as before.
///
/// Filtering is skipped when it would empty the catalog: an unfamiliar naming
/// scheme should degrade to an unranked list, never to a picker with nothing
/// in it.
fn models_from_openai_compatible_list(entries: Vec<OpenAiModelListEntry>) -> Vec<ModelInfo> {
    let conversational: Vec<&OpenAiModelListEntry> = entries
        .iter()
        .filter(|entry| openai_model_ranking::is_conversational_model(&entry.id))
        .collect();
    let retained: Vec<&OpenAiModelListEntry> = if conversational.is_empty() {
        entries.iter().collect()
    } else {
        conversational
    };

    let mut models: Vec<ModelInfo> = retained
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            model_info_from_catalog_entry(
                entry,
                openai_model_ranking::catalog_priority(&entry.id, index),
            )
        })
        .collect();
    models.sort_by_key(|model| model.priority);
    models
}

/// Curated metadata for a slug the bundled catalog already describes.
///
/// A `/v1/models` entry carries an id and nothing else, so building purely from
/// it discards the reasoning levels, context window, and tool settings the
/// bundled catalog defines — the engine then runs the model on fallback
/// metadata and warns that doing so can degrade performance.
fn bundled_model_info(slug: &str) -> Option<ModelInfo> {
    static BUNDLED: OnceLock<HashMap<String, ModelInfo>> = OnceLock::new();
    BUNDLED
        .get_or_init(|| {
            crate::bundled_models_response()
                .map(|response| {
                    response
                        .models
                        .into_iter()
                        .map(|model| (model.slug.clone(), model))
                        .collect()
                })
                .unwrap_or_default()
        })
        .get(slug)
        .cloned()
}

fn model_info_from_catalog_entry(entry: &OpenAiModelListEntry, priority: i32) -> ModelInfo {
    let known = bundled_model_info(&entry.id);
    let has_curated_metadata = known.is_some();
    let mut model = known.unwrap_or_else(|| model_info::model_info_from_slug(&entry.id));
    model.used_fallback_model_metadata = false;
    model.visibility = ModelVisibility::List;
    model.priority = priority;
    if let Some(name) = entry
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| (!has_curated_metadata).then(|| entry.id.clone()))
    {
        model.display_name = name;
    }
    if let Some(description) = entry
        .description
        .clone()
        .filter(|description| !description.trim().is_empty())
    {
        model.description = Some(description);
    }
    if let Some(context_window) = entry.context_length.or(entry.context_window) {
        model.context_window = Some(context_window);
        model.max_context_window = Some(context_window);
    }
    if entry.id == OPENROUTER_DEFAULT_MODEL {
        model.priority = i32::MIN;
    }
    apply_openrouter_capabilities(&mut model, entry);
    model
}

fn apply_openrouter_capabilities(model: &mut ModelInfo, entry: &OpenAiModelListEntry) {
    let supported = entry
        .supported_parameters
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let reasoning_supported = supported
        .iter()
        .any(|parameter| matches!(*parameter, "reasoning" | "include_reasoning"));
    let reasoning_required = entry
        .reasoning
        .as_ref()
        .and_then(|reasoning| reasoning.mandatory)
        .unwrap_or(false);
    if reasoning_supported || reasoning_required {
        model.supports_reasoning_summaries = true;
        if model.default_reasoning_level.is_none() {
            model.default_reasoning_level = Some(ReasoningEffort::Medium);
        }
        if model.supported_reasoning_levels.is_empty() {
            model.supported_reasoning_levels = default_reasoning_levels();
        }
    }
    if supported.contains(&"tools") {
        model.supports_parallel_tool_calls = true;
    }
}

fn default_reasoning_levels() -> Vec<ReasoningEffortPreset> {
    [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ]
    .into_iter()
    .map(|effort| ReasoningEffortPreset {
        effort: effort.clone(),
        description: effort.to_string(),
    })
    .collect()
}

#[derive(Debug, Deserialize)]
struct OpenAiModelsListResponse {
    data: Vec<OpenAiModelListEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelListEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    context_length: Option<i64>,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(default)]
    supported_parameters: Vec<String>,
    #[serde(default)]
    reasoning: Option<OpenRouterReasoningMetadata>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterReasoningMetadata {
    #[serde(default)]
    mandatory: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A first-party `/v1/models` body in the order the API returns it:
    /// account-creation order, oldest first, every family mixed together.
    fn openai_catalog_body() -> Vec<u8> {
        let ids = [
            "text-embedding-ada-002",
            "whisper-1",
            "gpt-3.5-turbo",
            "tts-1",
            "gpt-4",
            "davinci-002",
            "text-embedding-3-large",
            "gpt-4o",
            "omni-moderation-latest",
            "o3-mini",
            "gpt-4o-mini-search-preview",
            "o3",
            "gpt-4.1",
            "gpt-image-1",
            "o3-deep-research",
            "gpt-5",
            "gpt-5-mini",
            "gpt-realtime",
            "gpt-5-codex",
            "gpt-5-pro",
            "gpt-audio-mini",
            "sora-2-pro",
            "gpt-5.1",
            "gpt-5.1-codex-max",
            "gpt-5.2-2025-12-11",
            "gpt-5.2",
            "gpt-5.2-pro",
            "gpt-4o-mini-tts",
            "gpt-5.4",
            "gpt-5.4-mini",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.6-sol",
        ];
        let data: Vec<serde_json::Value> = ids.iter().map(|id| json!({ "id": id })).collect();
        json!({ "object": "list", "data": data })
            .to_string()
            .into_bytes()
    }

    /// The reported bug: the picker led with (and defaulted to) an embedding
    /// model while the frontier models sat at the bottom of the list.
    #[test]
    fn first_party_catalog_leads_with_the_most_capable_model() {
        let models = parse_model_catalog_response(&openai_catalog_body())
            .expect("openai catalog should parse");
        let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();

        assert_eq!(
            slugs.first(),
            Some(&"gpt-5.6-sol"),
            "the flagship model must lead the picker, got {slugs:?}"
        );
        assert_eq!(
            &slugs[..6],
            &[
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
            ]
        );
    }

    #[test]
    fn first_party_catalog_drops_models_that_cannot_chat() {
        let models = parse_model_catalog_response(&openai_catalog_body())
            .expect("openai catalog should parse");
        let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();

        for rejected in [
            "text-embedding-ada-002",
            "text-embedding-3-large",
            "whisper-1",
            "tts-1",
            "gpt-4o-mini-tts",
            "omni-moderation-latest",
            "gpt-image-1",
            "sora-2-pro",
            "gpt-realtime",
            "gpt-audio-mini",
            "davinci-002",
            "gpt-4o-mini-search-preview",
            "o3-deep-research",
        ] {
            assert!(
                !slugs.contains(&rejected),
                "{rejected} is not a chat model but survived: {slugs:?}"
            );
        }
        for kept in ["gpt-5.6-sol", "gpt-5-codex", "o3", "gpt-4.1", "gpt-3.5-turbo"] {
            assert!(slugs.contains(&kept), "{kept} should remain: {slugs:?}");
        }
    }

    /// A bare `/v1/models` entry carries only an id, so without reusing the
    /// bundled entry the engine runs the model on fallback metadata and warns
    /// that it "can degrade performance".
    #[test]
    fn first_party_catalog_keeps_curated_metadata_for_known_slugs() {
        let models = parse_model_catalog_response(&openai_catalog_body())
            .expect("openai catalog should parse");
        let sol = models
            .iter()
            .find(|model| model.slug == "gpt-5.6-sol")
            .expect("gpt-5.6-sol should be present");

        assert_eq!(sol.display_name, "GPT-5.6 Sol");
        assert!(!sol.used_fallback_model_metadata);
        assert!(
            sol.description
                .as_deref()
                .is_some_and(|text| text.contains("Flagship")),
            "curated description should survive, got {:?}",
            sol.description
        );
        assert!(
            !sol.supported_reasoning_levels.is_empty(),
            "curated reasoning levels should survive"
        );
    }

    /// A proxy may serve flat OpenAI ids beside namespaced vendor ids. Judging
    /// the catalog as a whole let one namespaced id wave the first-party half
    /// through unfiltered, putting an embedding model back on top.
    #[test]
    fn mixed_catalog_filters_first_party_entries_and_keeps_vendor_entries() {
        let body = json!({
            "object": "list",
            "data": [
                {"id": "text-embedding-ada-002"},
                {"id": "whisper-1"},
                {"id": "gpt-4o"},
                {"id": "anthropic/claude-sonnet-4"},
                {"id": "perplexity/sonar-deep-research"},
                {"id": "gpt-5.6-sol"},
            ]
        })
        .to_string();
        let models =
            parse_model_catalog_response(body.as_bytes()).expect("mixed catalog should parse");
        let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();

        assert_eq!(
            slugs.first(),
            Some(&"gpt-5.6-sol"),
            "first-party ranking still applies in a mixed catalog, got {slugs:?}"
        );
        for rejected in ["text-embedding-ada-002", "whisper-1"] {
            assert!(
                !slugs.contains(&rejected),
                "{rejected} should be filtered even beside namespaced ids: {slugs:?}"
            );
        }
        for kept in ["anthropic/claude-sonnet-4", "perplexity/sonar-deep-research"] {
            assert!(
                slugs.contains(&kept),
                "vendor-curated {kept} must survive: {slugs:?}"
            );
        }
    }

    /// A wholly-namespaced catalog is the OpenRouter case: no filtering, no
    /// scoring, upstream order preserved.
    #[test]
    fn aggregator_catalog_keeps_upstream_order() {
        let body = json!({
            "object": "list",
            "data": [
                {"id": "vendor/first"},
                {"id": "vendor/second"},
                {"id": "vendor/third"},
            ]
        })
        .to_string();
        let models =
            parse_model_catalog_response(body.as_bytes()).expect("aggregator catalog should parse");
        let slugs: Vec<&str> = models.iter().map(|model| model.slug.as_str()).collect();
        assert_eq!(slugs, vec!["vendor/first", "vendor/second", "vendor/third"]);
    }

    /// An unfamiliar catalog must degrade to an unranked list rather than to
    /// an empty picker.
    #[test]
    fn catalog_with_no_recognized_chat_models_is_left_intact() {
        let body = json!({
            "object": "list",
            "data": [{ "id": "tts-1" }, { "id": "whisper-1" }]
        })
        .to_string();
        let models =
            parse_model_catalog_response(body.as_bytes()).expect("catalog should still parse");
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn parses_codex_models_response() {
        let body = json!({
            "models": [{
                "slug": "gpt-test",
                "display_name": "gpt-test",
                "description": "desc",
                "default_reasoning_level": "medium",
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "medium", "description": "medium"},
                    {"effort": "high", "description": "high"}
                ],
                "shell_type": "shell_command",
                "visibility": "list",
                "supported_in_api": true,
                "priority": 1,
                "base_instructions": "base instructions",
                "supports_reasoning_summaries": false,
                "support_verbosity": false,
                "truncation_policy": {"mode": "bytes", "limit": 10_000},
                "supports_parallel_tool_calls": false,
                "context_window": 272_000,
                "experimental_supported_tools": [],
            }]
        });
        let models = parse_model_catalog_response(body.to_string().as_bytes())
            .expect("codex models response should parse");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].slug, "gpt-test");
    }

    #[test]
    fn parses_openai_compatible_models_list() {
        let body = json!({
            "object": "list",
            "data": [
                {
                    "id": "anthropic/claude-sonnet-4",
                    "name": "Anthropic: Claude Sonnet 4",
                    "description": "Strong general model",
                    "context_length": 200000
                },
                {
                    "id": "google/gemini-2.5-pro",
                    "name": "Google: Gemini 2.5 Pro"
                }
            ]
        });
        let models = parse_model_catalog_response(body.to_string().as_bytes())
            .expect("openai-compatible models response should parse");
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].slug, "anthropic/claude-sonnet-4");
        assert_eq!(models[0].display_name, "Anthropic: Claude Sonnet 4");
        assert_eq!(models[0].context_window, Some(200_000));
        assert!(!models[0].used_fallback_model_metadata);
        assert_eq!(models[1].slug, "google/gemini-2.5-pro");
        // Namespaced entries are unscored, so upstream order is what ranks
        // them; the absolute value of the key is an implementation detail.
        assert!(models[0].priority < models[1].priority);
    }

    #[test]
    fn openai_compatible_models_prefer_openrouter_auto_as_default() {
        let body = json!({
            "object": "list",
            "data": [
                {
                    "id": "aion-labs/aion-3.0-mini",
                    "name": "AionLabs: Aion-3.0-Mini",
                    "supported_parameters": ["tools"]
                },
                {
                    "id": "openrouter/auto",
                    "name": "OpenRouter Auto",
                    "supported_parameters": ["tools", "reasoning"]
                }
            ]
        });
        let models = parse_model_catalog_response(body.to_string().as_bytes())
            .expect("openrouter models should parse");
        let auto = models
            .iter()
            .find(|model| model.slug == OPENROUTER_DEFAULT_MODEL)
            .expect("openrouter/auto should be present");
        assert_eq!(auto.priority, i32::MIN);
    }

    #[test]
    fn openai_compatible_models_enable_reasoning_for_mandatory_openrouter_models() {
        let body = json!({
            "object": "list",
            "data": [{
                "id": "aion-labs/aion-3.0-mini",
                "name": "AionLabs: Aion-3.0-Mini",
                "context_length": 131072,
                "supported_parameters": [
                    "include_reasoning",
                    "reasoning",
                    "tools"
                ],
                "reasoning": {"mandatory": true}
            }]
        });
        let models = parse_model_catalog_response(body.to_string().as_bytes())
            .expect("openrouter model should parse");
        assert_eq!(models.len(), 1);
        assert!(models[0].supports_reasoning_summaries);
        assert_eq!(
            models[0].default_reasoning_level,
            Some(ReasoningEffort::Medium)
        );
        assert!(models[0].supports_parallel_tool_calls);
    }
}
