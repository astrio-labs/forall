use crate::hooks::browser_session::{
    session_has_browser_evidence, transcript_has_browser_evidence,
};

pub struct BrowserClaimInput<'a> {
    pub last_assistant_message: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub transcript_path: Option<&'a str>,
    pub project_root: Option<&'a std::path::Path>,
}

struct ClaimPattern {
    re: regex::Regex,
    label: &'static str,
}

fn claim_patterns() -> Vec<ClaimPattern> {
    vec![
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bverified (?:the |this )?(?:page |ui |login )?in the browser\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "verified in the browser",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\b(?:ui|browser) (?:check|test|verification?) pass(?:ed)?\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "UI/browser test passed",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\btested (?:the |this )?(?:page |ui )?in the browser\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "tested in the browser",
        },
        ClaimPattern {
            re: regex::Regex::new(
                r"(?i)\bconfirmed (?:the )?(?:page |ui ).{0,30}(?:in the browser|visually|works)\b",
            )
            .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "confirmed UI in browser",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bvisually verified\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "visually verified",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\blive browser session.{0,40}(?:confirm|verif|pass|success)")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "live browser session verified",
        },
        ClaimPattern {
            re: regex::Regex::new(
                r"(?i)\b(?:opened|loaded) .{0,60} in the browser.{0,40}(?:and )?(?:verified|confirmed|works|looks good)",
            )
            .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "opened and verified in browser",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bui pass\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "UI pass",
        },
        ClaimPattern {
            re: regex::Regex::new(r"(?i)\bverified\b[^.!\n]{0,50}\bin the browser\b")
                .unwrap_or_else(|err| panic!("invalid browser claim regex: {err}")),
            label: "verified in the browser",
        },
    ]
}

fn has_negated_claim(text: &str, match_index: usize) -> bool {
    let start = match_index.saturating_sub(90);
    let end = (match_index + 30).min(text.len());
    let window = text[start..end].to_lowercase();
    regex::Regex::new(
        r"(?i)\b(?:did not|didn't|do not|don't|cannot|can't|without|not|no live|never|did not run)\b",
    )
    .unwrap_or_else(|err| panic!("invalid claim guard regex: {err}"))
    .is_match(&window)
}

fn is_future_claim(text: &str, match_index: usize) -> bool {
    let start = match_index.saturating_sub(40);
    let end = (match_index + 50).min(text.len());
    let window = text[start..end].to_lowercase();
    regex::Regex::new(
        r"(?i)\b(?:i will|i'll|i am going to|i'm going to|next i|going to|plan to|about to|so i can)\b",
    )
    .unwrap_or_else(|err| panic!("invalid claim guard regex: {err}"))
    .is_match(&window)
}

pub fn find_browser_verification_claims(text: &str) -> Vec<String> {
    let mut claims = Vec::new();
    for pattern in claim_patterns() {
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

pub fn has_browser_verification_evidence(input: &BrowserClaimInput<'_>) -> bool {
    if let (Some(session_id), Some(root)) = (input.session_id, input.project_root)
        && session_has_browser_evidence(root, session_id.trim())
    {
        return true;
    }
    if let Some(path) = input.transcript_path
        && transcript_has_browser_evidence(path.trim())
    {
        return true;
    }
    false
}

pub fn validate_browser_verification_claims(input: &BrowserClaimInput<'_>) -> Option<String> {
    let message = input.last_assistant_message?.trim();
    if message.is_empty() {
        return None;
    }

    let claims = find_browser_verification_claims(message);
    if claims.is_empty() {
        return None;
    }

    if has_browser_verification_evidence(input) {
        return None;
    }

    let claim_summary = claims.join(", ");
    Some(format!(
        "Forall: assistant message claims browser/UI verification ({claim_summary}) \
         but no browser tools were used this session. \
         Use browser tools (browser_navigate, browser_snapshot, …) or revise the response to say the UI was not browser-tested."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::browser_session::record_browser_evidence;
    use crate::paths::FORALL_DIR;
    use crate::paths::ensure_dir;
    use tempfile::tempdir;

    #[test]
    fn detects_completion_claims() {
        let claims = find_browser_verification_claims(
            "I verified the login page in the browser and the UI pass looks good.",
        );
        assert!(claims.contains(&"verified in the browser".to_string()));
        assert!(claims.contains(&"UI pass".to_string()));
    }

    #[test]
    fn ignores_negated_and_future_claims() {
        assert!(
            find_browser_verification_claims(
                "Next I'm doing a quick UI check in the in-app browser so I can verify later."
            )
            .is_empty()
        );
        assert!(
            find_browser_verification_claims(
                "I did not run a live browser session to visually verify the page."
            )
            .is_empty()
        );
    }

    #[test]
    fn blocks_unsupported_claims() {
        let reason = validate_browser_verification_claims(&BrowserClaimInput {
            last_assistant_message: Some("Verified the page in the browser and UI pass."),
            session_id: Some("sess-1"),
            transcript_path: None,
            project_root: Some(std::path::Path::new("/tmp/unused")),
        });
        assert!(reason.is_some());
    }

    #[test]
    fn allows_claims_with_session_evidence() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        ensure_dir(&root.join(FORALL_DIR)).unwrap();
        record_browser_evidence(root, "sess-2", "browser_snapshot");
        let reason = validate_browser_verification_claims(&BrowserClaimInput {
            last_assistant_message: Some("Verified the page in the browser."),
            session_id: Some("sess-2"),
            transcript_path: None,
            project_root: Some(root),
        });
        assert!(reason.is_none());
    }
}
