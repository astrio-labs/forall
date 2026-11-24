use crate::mapping::schema::VerificationSummary;

pub struct TierClaimInput<'a> {
    pub last_assistant_message: Option<&'a str>,
    pub summary: Option<&'a VerificationSummary>,
}

struct ClaimPattern {
    re: regex::Regex,
    label: &'static str,
}

fn formal_proof_claim_patterns() -> Vec<ClaimPattern> {
    vec![
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bformally verified\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "formally verified",
        },
        ClaimPattern {
            re: regex::Regex::new(
                r"(?i)\bformal(?:ly)? (?:verification|proof)s? (?:passed|complete|succeeded|succeeds)\b",
            )
            .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "formal verification passed",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\b(?:mathematically|machine[- ]?checked) (?:verified|proved)\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "mathematically verified",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\b(?:dafny|theorem(?:ically)?) proved\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "Dafny proved",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bverus verified\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "Verus verified",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\b(?:fully|completely) proved\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "fully proved",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bproof(?:s)? (?:passed|verified|complete)\b")
                .unwrap_or_else(|err| panic!("invalid formal proof claim regex: {err}")),
            label: "proofs passed",
        },
    ]
}

fn has_negated_claim(text: &str, match_index: usize) -> bool {
    let start = match_index.saturating_sub(90);
    let end = (match_index + 40).min(text.len());
    let window = text[start..end].to_lowercase();
    regex::Regex::new(
        r"(?i)\b(?:did not|didn't|do not|don't|cannot|can't|without|not|never|no formal|not formally)\b",
    )
    .unwrap_or_else(|err| panic!("invalid claim guard regex: {err}"))
    .is_match(&window)
}

fn is_future_claim(text: &str, match_index: usize) -> bool {
    let start = match_index.saturating_sub(40);
    let end = (match_index + 50).min(text.len());
    let window = text[start..end].to_lowercase();
    regex::Regex::new(
        r"(?i)\b(?:i will|i'll|i am going to|i'm going to|next i|going to|plan to|about to|before claiming)\b",
    )
    .unwrap_or_else(|err| panic!("invalid claim guard regex: {err}"))
    .is_match(&window)
}

pub fn find_formal_proof_claims(text: &str) -> Vec<String> {
    let mut claims = Vec::new();
    for pattern in formal_proof_claim_patterns() {
        if let Some(found) = pattern.re.find(text) {
            if has_negated_claim(text, found.start()) || is_future_claim(text, found.start()) {
                continue;
            }
            if !claims.iter().any(|c| c == pattern.label) {
                claims.push(pattern.label.to_string());
            }
        }
    }
    claims
}

pub fn validate_tier_honesty_claims(input: &TierClaimInput<'_>) -> Option<String> {
    let message = input.last_assistant_message?.trim();
    if message.is_empty() {
        return None;
    }

    let claims = find_formal_proof_claims(message);
    if claims.is_empty() {
        return None;
    }

    let summary = input.summary?;

    if summary.proved_requirements > 0 {
        return None;
    }

    let claim_summary = claims.join(", ");
    Some(format!(
        "Forall: assistant message claims formal verification ({claim_summary}) \
         but check shows 0 proved requirements ({} property-tested, {} spec-tracked). \
         Revise the response to match the verification tier or add proofs before claiming formal verification.",
        summary.property_tested_requirements, summary.spec_tracked_requirements
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::VerificationSummary;

    fn summary(proved: usize, property: usize, spec: usize) -> VerificationSummary {
        VerificationSummary {
            total_requirements: proved + property + spec,
            proved_requirements: proved,
            property_tested_requirements: property,
            spec_tracked_requirements: spec,
        }
    }

    #[test]
    fn detects_formal_verification_claims() {
        let claims = find_formal_proof_claims(
            "The clamp logic is formally verified and formal verification passed.",
        );
        assert!(claims.contains(&"formally verified".to_string()));
        assert!(claims.contains(&"formal verification passed".to_string()));
    }

    #[test]
    fn ignores_negated_formal_claims() {
        assert!(
            find_formal_proof_claims("This is not formally verified — spec-tracked only.")
                .is_empty()
        );
    }

    #[test]
    fn blocks_formal_claim_when_zero_proved() {
        let s = summary(0, 0, 1);
        let reason = validate_tier_honesty_claims(&TierClaimInput {
            last_assistant_message: Some("The code is formally verified."),
            summary: Some(&s),
        });
        assert!(reason.is_some());
        assert!(reason.unwrap().contains("0 proved"));
    }

    #[test]
    fn allows_formal_claim_when_proved() {
        let s = summary(1, 0, 0);
        let reason = validate_tier_honesty_claims(&TierClaimInput {
            last_assistant_message: Some("The code is formally verified."),
            summary: Some(&s),
        });
        assert!(reason.is_none());
    }
}
