use super::ContextualUserFragment;

const COMMIT_ATTRIBUTION_OPEN_TAG: &str = "<commit_attribution>";
const COMMIT_ATTRIBUTION_CLOSE_TAG: &str = "</commit_attribution>";

/// Trailer identity GitHub resolves to the astrio-forall GitHub App's bot
/// user. Must stay byte-identical to `FORALL_COAUTHOR_TRAILER` in the
/// desktop's `coauthor.ts`, or the two surfaces would attribute to
/// different identities.
pub(crate) const FORALL_COAUTHOR_TRAILER: &str =
    "Co-authored-by: Forall <311994675+astrio-forall[bot]@users.noreply.github.com>";

/// Instructs the model to credit Forall on commits it creates. Injected as a
/// developer block, gated by the `credit_forall_on_commits` config key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitAttributionInstructions;

impl ContextualUserFragment for CommitAttributionInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (COMMIT_ATTRIBUTION_OPEN_TAG, COMMIT_ATTRIBUTION_CLOSE_TAG)
    }

    fn body(&self) -> String {
        format!(
            "When you create a git commit, end the commit message with this trailer, \
             separated from the rest of the message by a blank line:\n\n\
             {FORALL_COAUTHOR_TRAILER}\n\n\
             Skip it only when the message already contains that trailer."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// The rendered block pins the exact identity the GitHub App resolves;
    /// a drifted email silently stops attribution on every commit.
    #[test]
    fn body_carries_the_pinned_trailer_as_its_own_paragraph() {
        let rendered = CommitAttributionInstructions.render();
        assert!(rendered.starts_with(COMMIT_ATTRIBUTION_OPEN_TAG));
        assert!(rendered.ends_with(COMMIT_ATTRIBUTION_CLOSE_TAG));
        assert!(rendered.contains(&format!("\n\n{FORALL_COAUTHOR_TRAILER}\n\n")));
        assert_eq!(
            FORALL_COAUTHOR_TRAILER,
            "Co-authored-by: Forall <311994675+astrio-forall[bot]@users.noreply.github.com>"
        );
    }
}
