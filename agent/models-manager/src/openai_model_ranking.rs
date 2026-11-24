//! Chat-capability filtering and power ordering for first-party OpenAI catalogs.
//!
//! `/v1/models` answers in account-creation order and lists every family the
//! key can reach — embeddings, speech, image, video, moderation. Consumed
//! verbatim that makes `text-embedding-ada-002` the first entry, and the first
//! picker-visible entry is what [`ModelPreset::mark_default_by_picker_visibility`]
//! marks as the default model. So the raw catalog both buries the frontier
//! models and hands the session a model that cannot hold a conversation.
//!
//! Ordering is generation-first, then tier within the generation: a newer
//! generation's base model outranks an older generation's `pro`. Unscorable
//! slugs sort below everything scored and keep their upstream order, so a
//! proxy serving non-OpenAI names degrades to today's behavior instead of
//! being shuffled arbitrarily.
//!
//! [`ModelPreset::mark_default_by_picker_visibility`]: forall_protocol::openai_models::ModelPreset::mark_default_by_picker_visibility

/// Families the agent cannot drive as a conversational model.
///
/// Matched against hyphen/period-delimited slug tokens rather than as
/// substrings, so a future `gpt-6-image-reasoning` style name cannot be
/// excluded by an accidental match inside an unrelated word.
const NON_CONVERSATIONAL_TOKENS: &[&str] = &[
    "embedding",
    "embeddings",
    "moderation",
    "tts",
    "transcribe",
    "whisper",
    "realtime",
    "audio",
    "image",
    "sora",
    "video",
    "instruct",
    "davinci",
    "babbage",
    "curie",
    // `-search-api` / `-search-preview` and `-deep-research` are retrieval
    // endpoints with their own request shape, not general chat models.
    "search",
    "research",
];

/// Highest score any slug can reach; keeps derived priorities non-negative.
const MAX_SCORE: i32 = 1_000;

/// Score for slugs from a family this build does not know how to rank.
///
/// Below every scored OpenAI model so a recognized frontier model always wins,
/// but identical across unknowns so their upstream order is what breaks the tie.
const UNSCORED: i32 = 100;

/// True for an aggregator entry — OpenRouter and LiteLLM namespace every id as
/// `vendor/model`.
///
/// Those catalogs are curated by the vendor and use naming this module has no
/// authority over: `perplexity/sonar-deep-research` is a chat model despite
/// reading as a research endpoint under first-party naming. They are neither
/// filtered nor scored here.
fn is_aggregator_entry(slug: &str) -> bool {
    slug.contains('/')
}

/// True when `slug` names a model the agent can hold a conversation with.
pub(crate) fn is_conversational_model(slug: &str) -> bool {
    is_aggregator_entry(slug)
        || !slug
            .split(['-', '.', '_'])
            .any(|token| NON_CONVERSATIONAL_TOKENS.contains(&token.to_ascii_lowercase().as_str()))
}

/// Sort key for `slug`; smaller sorts first, so smaller is more capable.
///
/// `index` is the model's position in the upstream response and breaks ties
/// between equally scored slugs. Dated snapshots sort just below the floating
/// alias they pin (`gpt-5.2-2025-12-11` under `gpt-5.2`) so the picker leads
/// with the name that keeps working.
pub(crate) fn catalog_priority(slug: &str, index: usize) -> i32 {
    // Clamped because the score feeds fixed-width arithmetic below and the slug
    // is provider-controlled: `MAX_SCORE` must stay the ceiling no matter what
    // an upstream id claims to be.
    let score = capability_score(slug)
        .unwrap_or(UNSCORED)
        .clamp(0, MAX_SCORE);
    let dated = i32::from(is_dated_snapshot(slug));
    let index = i32::try_from(index).unwrap_or(i32::MAX).clamp(0, 999);
    (MAX_SCORE - score) * 2_000 + dated * 1_000 + index
}

/// Relative capability of `slug`; larger is more capable.
fn capability_score(slug: &str) -> Option<i32> {
    if is_aggregator_entry(slug) {
        return None;
    }
    Some(generation_score(slug)? * 10 + tier_score(slug))
}

/// Highest generation this scoring accepts.
///
/// Two digits covers every released and foreseeable OpenAI line, and bounding
/// it here is what keeps `generation * 10` in [`capability_score`] from
/// overflowing on an id like `gpt-999999999`.
const MAX_GENERATION: i32 = 99;

/// Model generation, scaled so `5.6` outranks `5.5` outranks `5`.
fn generation_score(slug: &str) -> Option<i32> {
    let slug = slug.to_ascii_lowercase();
    if let Some(rest) = slug.strip_prefix("gpt-") {
        let head = rest.split('-').next().unwrap_or(rest);
        return match head {
            // `4o` is the omni revision of 4 and outranks plain `gpt-4`;
            // neither parses as a decimal.
            "4o" => Some(40),
            "4" => Some(38),
            other => parse_decimal_generation(other),
        };
    }
    // The o-series reasoning models sit between the gpt-4 and gpt-5 lines.
    if let Some(rest) = slug.strip_prefix('o')
        && let Some(generation) = rest
            .split(['-', '.'])
            .next()
            .and_then(|head| head.parse::<i32>().ok())
        && (1..=9).contains(&generation)
    {
        return Some(43 + generation);
    }
    None
}

/// Parses `5`, `5.6`, `4.1` into a comparable integer, so a generation this
/// build has never heard of still slots in above its predecessors.
fn parse_decimal_generation(head: &str) -> Option<i32> {
    let mut parts = head.split('.');
    let major = parts.next()?.parse::<i32>().ok()?;
    let minor = match parts.next() {
        Some(minor) => minor.parse::<i32>().ok()?,
        None => 0,
    };
    if parts.next().is_some() || !(0..=9).contains(&minor) || !(0..=MAX_GENERATION).contains(&major)
    {
        return None;
    }
    Some(major * 10 + minor)
}

/// Tier within a generation. Checked most-specific first: `codex-max` is a
/// `max`, `codex-mini` is a `mini`, and a bare generation is the flagship.
fn tier_score(slug: &str) -> i32 {
    let tokens: Vec<String> = slug
        .split(['-', '.'])
        .map(str::to_ascii_lowercase)
        .collect();
    let has = |needle: &str| tokens.iter().any(|token| token == needle);
    if has("nano") {
        0
    } else if has("mini") {
        1
    } else if has("luna") {
        2
    } else if has("chat") {
        3
    } else if has("pro") {
        9
    } else if has("max") {
        8
    } else if has("sol") {
        7
    } else if has("terra") {
        5
    } else if has("codex") {
        4
    } else {
        6
    }
}

/// True for pinned snapshots such as `gpt-5.2-2025-12-11`.
fn is_dated_snapshot(slug: &str) -> bool {
    let mut parts = slug.rsplit('-');
    let (Some(day), Some(month), Some(year)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact failure that motivated this module: the raw catalog made an
    /// embedding model the first — and therefore default — picker entry.
    #[test]
    fn non_conversational_families_are_rejected() {
        for slug in [
            "text-embedding-ada-002",
            "text-embedding-3-large",
            "whisper-1",
            "tts-1-hd",
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "omni-moderation-latest",
            "gpt-image-1.5",
            "chatgpt-image-latest",
            "sora-2-pro",
            "gpt-realtime-mini",
            "gpt-audio",
            "davinci-002",
            "babbage-002",
            "gpt-3.5-turbo-instruct",
            "gpt-5-search-api",
            "gpt-4o-mini-search-preview",
            "o3-deep-research",
        ] {
            assert!(!is_conversational_model(slug), "{slug} should be rejected");
        }
    }

    #[test]
    fn conversational_models_are_kept() {
        for slug in [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.5",
            "gpt-5.4-mini",
            "gpt-5.3-codex",
            "gpt-5.1-codex-max",
            "gpt-5-pro",
            "gpt-4.1",
            "gpt-4o",
            "o3",
            "o4-mini",
            "gpt-3.5-turbo",
        ] {
            assert!(is_conversational_model(slug), "{slug} should be kept");
        }
    }

    /// Aggregator naming is the vendor's, not ours: these read as excluded
    /// families under first-party rules but are ordinary chat models upstream.
    #[test]
    fn namespaced_third_party_slugs_are_kept() {
        for slug in [
            "anthropic/claude-sonnet-4",
            "google/gemini-2.5-pro",
            "openrouter/auto",
            "perplexity/sonar-deep-research",
            "x-ai/grok-4-image-reasoning",
            "openai/gpt-4o-audio-preview",
        ] {
            assert!(is_conversational_model(slug), "{slug} should be kept");
        }
    }

    /// Aggregator entries carry no OpenAI generation to reason about, so they
    /// stay unscored and fall back to upstream order.
    #[test]
    fn namespaced_slugs_are_never_scored() {
        for slug in ["openai/gpt-5.6-sol", "anthropic/claude-sonnet-4"] {
            assert_eq!(capability_score(slug), None, "{slug} should be unscored");
        }
    }

    /// A provider-controlled id must not overflow the fixed-width arithmetic
    /// behind the sort key.
    #[test]
    fn absurd_generations_do_not_overflow_or_outrank_real_models() {
        for slug in [
            "gpt-999999999",
            "gpt-2147483647",
            "gpt-99999999999999999999",
            "gpt-100",
            "o99999999",
        ] {
            let priority = catalog_priority(slug, 0);
            assert!(
                priority > catalog_priority("gpt-5.6-sol", 0),
                "{slug} must not outrank a real frontier model"
            );
        }
        // The highest accepted generation still scores, and stays in bounds.
        assert!(capability_score("gpt-99.9").is_some());
        assert!(catalog_priority("gpt-99.9", 0) >= 0);
    }

    fn ordered(slugs: &[&str]) -> Vec<String> {
        let mut ranked: Vec<(i32, &str)> = slugs
            .iter()
            .enumerate()
            .map(|(index, slug)| (catalog_priority(slug, index), *slug))
            .collect();
        ranked.sort_by_key(|(priority, _)| *priority);
        ranked
            .into_iter()
            .map(|(_, slug)| slug.to_string())
            .collect()
    }

    #[test]
    fn most_powerful_model_sorts_first() {
        // Deliberately seeded in OpenAI's creation order, oldest first.
        let ordered = ordered(&[
            "gpt-3.5-turbo",
            "gpt-4o",
            "gpt-4.1",
            "o3",
            "gpt-5",
            "gpt-5-pro",
            "gpt-5.1-codex-max",
            "gpt-5.2",
            "gpt-5.4-mini",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.6-sol",
        ]);
        assert_eq!(ordered.first().map(String::as_str), Some("gpt-5.6-sol"));
        assert_eq!(ordered.last().map(String::as_str), Some("gpt-3.5-turbo"));
    }

    #[test]
    fn generation_outranks_tier() {
        let ordered = ordered(&["gpt-5.2-pro", "gpt-5.4"]);
        assert_eq!(ordered, vec!["gpt-5.4", "gpt-5.2-pro"]);
    }

    #[test]
    fn tier_orders_within_a_generation() {
        let ordered = ordered(&[
            "gpt-5.1-codex-mini",
            "gpt-5.1-codex",
            "gpt-5.1",
            "gpt-5.1-codex-max",
        ]);
        assert_eq!(
            ordered,
            vec![
                "gpt-5.1-codex-max",
                "gpt-5.1",
                "gpt-5.1-codex",
                "gpt-5.1-codex-mini",
            ]
        );
    }

    #[test]
    fn forall_frontier_variants_order_sol_terra_luna() {
        let ordered = ordered(&["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"]);
        assert_eq!(ordered, vec!["gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"]);
    }

    #[test]
    fn floating_alias_outranks_its_dated_snapshot() {
        let ordered = ordered(&["gpt-5.2-2025-12-11", "gpt-5.2"]);
        assert_eq!(ordered, vec!["gpt-5.2", "gpt-5.2-2025-12-11"]);
        assert!(is_dated_snapshot("gpt-5.2-2025-12-11"));
        assert!(!is_dated_snapshot("gpt-5.2"));
        assert!(!is_dated_snapshot("gpt-4.1-nano"));
    }

    /// A generation newer than anything this build knows must lead the list
    /// without a code change.
    #[test]
    fn unreleased_generations_rank_above_known_ones() {
        let ordered = ordered(&["gpt-5.6-sol", "gpt-7.1"]);
        assert_eq!(ordered, vec!["gpt-7.1", "gpt-5.6-sol"]);
    }

    #[test]
    fn unscored_slugs_sort_last_in_upstream_order() {
        let ordered = ordered(&["mystery-model-a", "mystery-model-b", "gpt-4o"]);
        assert_eq!(
            ordered,
            vec!["gpt-4o", "mystery-model-a", "mystery-model-b"]
        );
    }

    #[test]
    fn o_series_sits_between_gpt_4_and_gpt_5() {
        let ordered = ordered(&["gpt-4.1", "o3", "gpt-5"]);
        assert_eq!(ordered, vec!["gpt-5", "o3", "gpt-4.1"]);
    }
}
