use forall_model_provider_info::GEMINI_PRO_MODEL_ID;
use forall_models_manager::model_info::BASE_INSTRUCTIONS;
use forall_protocol::config_types::ReasoningSummary;
use forall_protocol::openai_models::ApplyPatchToolType;
use forall_protocol::openai_models::ConfigShellToolType;
use forall_protocol::openai_models::InputModality;
use forall_protocol::openai_models::ModelInfo;
use forall_protocol::openai_models::ModelVisibility;
use forall_protocol::openai_models::ModelsResponse;
use forall_protocol::openai_models::ReasoningEffort;
use forall_protocol::openai_models::ReasoningEffortPreset;
use forall_protocol::openai_models::TruncationPolicyConfig;
use forall_protocol::openai_models::WebSearchToolType;

pub(crate) const GEMINI_FLASH_MODEL_ID: &str = "gemini-2.5-flash";
pub(crate) const GEMINI_FLASH_LITE_MODEL_ID: &str = "gemini-2.5-flash-lite";

/// Every bundled Gemini 2.5 model accepts a 1,048,576-token input context.
const GEMINI_CONTEXT_WINDOW: i64 = 1_048_576;
const GEMINI_EFFECTIVE_CONTEXT_WINDOW_PERCENT: i64 = 95;
const GEMINI_TRUNCATION_TOKEN_LIMIT: i64 = 10_000;

/// Bundled static catalog for the Gemini provider.
///
/// Gemini exposes no OpenAI-compatible `/models` endpoint (its own
/// `models.list` is a different shape), so the catalog ships with the binary
/// instead of being fetched remotely. Only generally-available 2.5 slugs are
/// listed: a preview slug would be a metadata claim that expires.
pub(crate) fn static_model_catalog() -> ModelsResponse {
    with_default_only_service_tier(ModelsResponse {
        models: vec![
            gemini_model(
                GEMINI_PRO_MODEL_ID,
                "Gemini 2.5 Pro",
                "Most capable Gemini model for complex reasoning and coding.",
                /*priority*/ 0,
            ),
            gemini_model(
                GEMINI_FLASH_MODEL_ID,
                "Gemini 2.5 Flash",
                "Balanced Gemini model for everyday coding and agentic work.",
                /*priority*/ 1,
            ),
            gemini_model(
                GEMINI_FLASH_LITE_MODEL_ID,
                "Gemini 2.5 Flash-Lite",
                "Fastest Gemini model for simple, latency-sensitive tasks.",
                /*priority*/ 2,
            ),
        ],
    })
}

/// Gemini has no OpenAI-style service tiers; clear them on every model.
pub(crate) fn with_default_only_service_tier(mut catalog: ModelsResponse) -> ModelsResponse {
    for model in &mut catalog.models {
        model.additional_speed_tiers.clear();
        model.service_tiers.clear();
        model.default_service_tier = None;
    }
    catalog
}

fn gemini_reasoning_levels() -> Vec<ReasoningEffortPreset> {
    vec![
        ReasoningEffortPreset {
            effort: ReasoningEffort::Low,
            description: "Fast responses with a small thinking budget".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "Balances speed and thinking depth for everyday tasks".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "Larger thinking budget for complex problems".to_string(),
        },
    ]
}

fn gemini_model(slug: &str, display_name: &str, description: &str, priority: i32) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
        description: Some(description.to_string()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        supported_reasoning_levels: gemini_reasoning_levels(),
        shell_type: ConfigShellToolType::ShellCommand,
        visibility: ModelVisibility::List,
        supported_in_api: true,
        priority,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        // Branding is re-applied by `with_config_overrides` when the model is
        // resolved, matching how bundled models supply the standard prompt.
        base_instructions: BASE_INSTRUCTIONS.to_string(),
        model_messages: None,
        // Gemini 2.5 models think by default and return thought summaries when
        // `includeThoughts` is set, which is what gates `thinkingConfig` in
        // `build_gemini_request`.
        supports_reasoning_summaries: true,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        // Freeform apply_patch is mapped onto a single-string-parameter function
        // declaration by `create_tools_json_for_gemini_api`, so advertising it
        // here does not create a tool that vanishes on the wire.
        apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::tokens(GEMINI_TRUNCATION_TOKEN_LIMIT),
        // Gemini may return several `functionCall` parts in one response and has
        // no toggle to prevent it.
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(GEMINI_CONTEXT_WINDOW),
        max_context_window: Some(GEMINI_CONTEXT_WINDOW),
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: GEMINI_EFFECTIVE_CONTEXT_WINDOW_PERCENT,
        experimental_supported_tools: Vec::new(),
        input_modalities: vec![InputModality::Text, InputModality::Image],
        used_fallback_model_metadata: false,
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
    }
}

#[cfg(test)]
mod tests {
    use forall_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn catalog_lists_gemini_models_by_priority() {
        let catalog = static_model_catalog();

        let slugs = catalog
            .models
            .iter()
            .map(|model| model.slug.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            slugs,
            vec![
                GEMINI_PRO_MODEL_ID,
                GEMINI_FLASH_MODEL_ID,
                GEMINI_FLASH_LITE_MODEL_ID,
            ]
        );
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.priority)
                .collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn gemini_models_use_one_million_token_context_window() {
        for model in static_model_catalog().models {
            assert_eq!(model.context_window, Some(GEMINI_CONTEXT_WINDOW));
            assert_eq!(model.max_context_window, Some(GEMINI_CONTEXT_WINDOW));
            assert_eq!(
                model.effective_context_window_percent,
                GEMINI_EFFECTIVE_CONTEXT_WINDOW_PERCENT
            );
        }
    }

    #[test]
    fn gemini_models_support_reasoning_and_parallel_tool_calls() {
        for model in static_model_catalog().models {
            assert!(model.supports_reasoning_summaries);
            assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Medium));
            assert_eq!(
                model
                    .supported_reasoning_levels
                    .iter()
                    .map(|preset| preset.effort.clone())
                    .collect::<Vec<_>>(),
                vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ]
            );
            assert!(model.supports_parallel_tool_calls);
            assert_eq!(
                model.apply_patch_tool_type,
                Some(ApplyPatchToolType::Freeform)
            );
            assert_eq!(
                model.truncation_policy,
                TruncationPolicyConfig::tokens(GEMINI_TRUNCATION_TOKEN_LIMIT)
            );
            assert_eq!(model.visibility, ModelVisibility::List);
            assert!(model.supported_in_api);
            assert!(!model.base_instructions.is_empty());
            assert_eq!(
                model.input_modalities,
                vec![InputModality::Text, InputModality::Image]
            );
        }
    }

    #[test]
    fn gemini_models_only_allow_default_service_tier() {
        for model in static_model_catalog().models {
            assert_eq!(model.additional_speed_tiers, Vec::<String>::new());
            assert_eq!(model.service_tiers, Vec::new());
            assert_eq!(model.default_service_tier, None);
            assert_eq!(
                model.service_tier_for_request(Some("priority".to_string())),
                None
            );
            assert_eq!(
                model
                    .service_tier_for_request(Some(SERVICE_TIER_DEFAULT_REQUEST_VALUE.to_string())),
                None
            );
        }
    }
}
