//! Bundled catalog for Claude models served by `bedrock-runtime`.
//!
//! Metadata mirrors the direct-Anthropic catalog (`crate::anthropic::catalog`)
//! because the model is the same model and the request/response adapter is the
//! same adapter: only the transport differs. Anything claimed here that the
//! Messages adapter cannot actually round-trip would be a lie regardless of
//! which front door the request went through.

use forall_model_provider_info::BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID;
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

pub(super) const BEDROCK_ANTHROPIC_CLAUDE_SONNET_MODEL_ID: &str = "anthropic.claude-sonnet-4-6";
pub(super) const BEDROCK_ANTHROPIC_CLAUDE_HAIKU_MODEL_ID: &str = "anthropic.claude-haiku-4-5";

/// Geographic prefixes AWS uses for cross-region inference profiles.
///
/// A profile id is the base model id with one of these prepended
/// (`us.anthropic.claude-opus-4-6`), and it routes the request across the
/// regions of that geography. Newer Claude models are frequently *only*
/// invocable through a profile, so these must resolve to real metadata rather
/// than the unknown-model fallback.
const INFERENCE_PROFILE_PREFIXES: [&str; 3] = ["us.", "eu.", "apac."];

/// Claude models are exposed with a conservative 200k-token context window.
const CLAUDE_CONTEXT_WINDOW: i64 = 200_000;
const CLAUDE_EFFECTIVE_CONTEXT_WINDOW_PERCENT: i64 = 95;
const CLAUDE_TRUNCATION_TOKEN_LIMIT: i64 = 10_000;

/// Bundled static catalog for the Claude-on-Bedrock provider.
///
/// Bedrock has no OpenAI-compatible `/models` endpoint, so the catalog ships
/// with the binary.
///
/// Slugs follow the `{vendor}.{model}` convention the Mantle catalog already
/// uses (`openai.gpt-5.5`). Real AWS on-demand ids additionally carry a release
/// date and version suffix (`...-20260101-v1:0`); those resolve to the same
/// metadata because model lookup is a longest-*prefix* match, so a fully
/// qualified id is a superset of the bundled slug. Each model also gets hidden
/// entries for the cross-region inference-profile prefixes — invocable and
/// correctly described, but kept out of the picker so choosing a geography stays
/// a deliberate act rather than four near-identical rows.
pub(super) fn static_model_catalog() -> ModelsResponse {
    let base = [
        (
            BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID,
            "Claude Opus 4.6 (Bedrock)",
            "Most capable Claude model for complex reasoning and coding.",
        ),
        (
            BEDROCK_ANTHROPIC_CLAUDE_SONNET_MODEL_ID,
            "Claude Sonnet 4.6 (Bedrock)",
            "Balanced Claude model for everyday coding and agentic work.",
        ),
        (
            BEDROCK_ANTHROPIC_CLAUDE_HAIKU_MODEL_ID,
            "Claude Haiku 4.5 (Bedrock)",
            "Fastest Claude model for simple, latency-sensitive tasks.",
        ),
    ];

    let mut models = Vec::with_capacity(base.len() * (1 + INFERENCE_PROFILE_PREFIXES.len()));
    for (index, (slug, display_name, description)) in base.into_iter().enumerate() {
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let priority = index as i32;
        models.push(claude_bedrock_model(
            slug,
            display_name,
            description,
            priority,
            ModelVisibility::List,
        ));
        for prefix in INFERENCE_PROFILE_PREFIXES {
            models.push(claude_bedrock_model(
                &format!("{prefix}{slug}"),
                &format!("{display_name} [{}]", prefix.trim_end_matches('.')),
                description,
                priority,
                // Resolvable by id, absent from the picker.
                ModelVisibility::Hide,
            ));
        }
    }

    with_default_only_service_tier(ModelsResponse { models })
}

/// Bedrock exposes no OpenAI-style service tiers; clear them on every model.
pub(super) fn with_default_only_service_tier(mut catalog: ModelsResponse) -> ModelsResponse {
    for model in &mut catalog.models {
        model.additional_speed_tiers.clear();
        model.service_tiers.clear();
        model.default_service_tier = None;
    }
    catalog
}

fn claude_reasoning_levels() -> Vec<ReasoningEffortPreset> {
    vec![
        ReasoningEffortPreset {
            effort: ReasoningEffort::Low,
            description: "Fast responses with lighter reasoning".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::Medium,
            description: "Balances speed and reasoning depth for everyday tasks".to_string(),
        },
        ReasoningEffortPreset {
            effort: ReasoningEffort::High,
            description: "Greater reasoning depth for complex problems".to_string(),
        },
    ]
}

fn claude_bedrock_model(
    slug: &str,
    display_name: &str,
    description: &str,
    priority: i32,
    visibility: ModelVisibility,
) -> ModelInfo {
    ModelInfo {
        slug: slug.to_string(),
        display_name: display_name.to_string(),
        description: Some(description.to_string()),
        default_reasoning_level: Some(ReasoningEffort::Medium),
        // Extended thinking reaches Bedrock through the same `thinking` body
        // field the direct endpoint uses, so effort presets are honest here.
        supported_reasoning_levels: claude_reasoning_levels(),
        shell_type: ConfigShellToolType::ShellCommand,
        visibility,
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
        supports_reasoning_summaries: true,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        // The Messages adapter advertises freeform tools as single-string
        // object tools and unwraps the resulting call back into
        // `CustomToolCall`, so `Freeform` is what it can actually round-trip.
        apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::tokens(CLAUDE_TRUNCATION_TOKEN_LIMIT),
        supports_parallel_tool_calls: true,
        supports_image_detail_original: false,
        context_window: Some(CLAUDE_CONTEXT_WINDOW),
        max_context_window: Some(CLAUDE_CONTEXT_WINDOW),
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: CLAUDE_EFFECTIVE_CONTEXT_WINDOW_PERCENT,
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
    fn catalog_lists_the_three_claude_models_in_the_picker() {
        let listed = static_model_catalog()
            .models
            .into_iter()
            .filter(|model| model.visibility == ModelVisibility::List)
            .map(|model| model.slug)
            .collect::<Vec<_>>();

        assert_eq!(
            listed,
            vec![
                BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID,
                BEDROCK_ANTHROPIC_CLAUDE_SONNET_MODEL_ID,
                BEDROCK_ANTHROPIC_CLAUDE_HAIKU_MODEL_ID,
            ]
        );
    }

    #[test]
    fn catalog_slugs_use_the_bedrock_vendor_prefix() {
        for model in static_model_catalog().models {
            assert!(
                model.slug.contains("anthropic.claude-"),
                "{} is not a Bedrock-shaped model id",
                model.slug
            );
        }
    }

    #[test]
    fn cross_region_inference_profiles_are_present_but_hidden() {
        let catalog = static_model_catalog();

        for prefix in INFERENCE_PROFILE_PREFIXES {
            let slug = format!("{prefix}{BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID}");
            let model = catalog
                .models
                .iter()
                .find(|model| model.slug == slug)
                .unwrap_or_else(|| panic!("catalog should include {slug}"));

            assert_eq!(
                model.visibility,
                ModelVisibility::Hide,
                "{slug} must resolve without cluttering the picker"
            );
            assert_eq!(model.context_window, Some(CLAUDE_CONTEXT_WINDOW));
            assert!(model.supports_reasoning_summaries);
        }
    }

    #[test]
    fn claude_models_use_200k_context_window() {
        for model in static_model_catalog().models {
            assert_eq!(model.context_window, Some(CLAUDE_CONTEXT_WINDOW));
            assert_eq!(model.max_context_window, Some(CLAUDE_CONTEXT_WINDOW));
            assert_eq!(
                model.effective_context_window_percent,
                CLAUDE_EFFECTIVE_CONTEXT_WINDOW_PERCENT
            );
        }
    }

    #[test]
    fn claude_models_support_reasoning_freeform_patches_and_parallel_tool_calls() {
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
                TruncationPolicyConfig::tokens(CLAUDE_TRUNCATION_TOKEN_LIMIT)
            );
            assert!(model.supported_in_api);
            assert!(!model.base_instructions.is_empty());
            assert_eq!(
                model.input_modalities,
                vec![InputModality::Text, InputModality::Image]
            );
        }
    }

    /// The bundled metadata must match the direct-Anthropic catalog: same model,
    /// same Messages adapter, so any divergence would mean one of the two is
    /// misdescribing what Forall can do.
    #[test]
    fn bedrock_metadata_matches_the_direct_anthropic_catalog() {
        let direct = crate::anthropic::static_model_catalog_for_test();
        let direct_opus = direct
            .models
            .iter()
            .find(|model| model.slug == "claude-opus-4-6")
            .expect("direct catalog should include Opus");
        let bedrock = static_model_catalog();
        let bedrock_opus = bedrock
            .models
            .iter()
            .find(|model| model.slug == BEDROCK_ANTHROPIC_CLAUDE_OPUS_MODEL_ID)
            .expect("bedrock catalog should include Opus");

        assert_eq!(bedrock_opus.context_window, direct_opus.context_window);
        assert_eq!(
            bedrock_opus.max_context_window,
            direct_opus.max_context_window
        );
        assert_eq!(
            bedrock_opus.effective_context_window_percent,
            direct_opus.effective_context_window_percent
        );
        assert_eq!(
            bedrock_opus.apply_patch_tool_type,
            direct_opus.apply_patch_tool_type
        );
        assert_eq!(
            bedrock_opus.supports_parallel_tool_calls,
            direct_opus.supports_parallel_tool_calls
        );
        assert_eq!(
            bedrock_opus.supports_reasoning_summaries,
            direct_opus.supports_reasoning_summaries
        );
        assert_eq!(
            bedrock_opus.supported_reasoning_levels,
            direct_opus.supported_reasoning_levels
        );
        assert_eq!(bedrock_opus.shell_type, direct_opus.shell_type);
        assert_eq!(bedrock_opus.input_modalities, direct_opus.input_modalities);
        assert_eq!(
            bedrock_opus.truncation_policy,
            direct_opus.truncation_policy
        );
    }

    #[test]
    fn claude_models_only_allow_default_service_tier() {
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
