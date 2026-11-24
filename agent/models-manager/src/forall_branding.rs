pub const FORALL_IDENTITY_HEADER: &str = "You are Forall, Astrio's Forall agent. You and the user share the same workspace and collaborate to achieve the user's goals.";

const REPLACEMENTS: &[(&str, &str)] = &[
    (
        "You are Forall, a coding agent based on GPT-5. You and the user share one workspace, and your job is to collaborate with them until their goal is genuinely handled.",
        "You are Forall, Astrio's Forall agent. You and the user share one workspace, and your job is to collaborate with them until their goal is genuinely handled.",
    ),
    (
        "You are Forall, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.",
        FORALL_IDENTITY_HEADER,
    ),
    (
        "You are Forall, based on GPT-5. You are running as a coding agent in the Forall CLI on a user's computer.",
        "You are Forall, Astrio's Forall agent. You are running in the Forall CLI on a user's computer.",
    ),
    (
        "You are GPT-5.2 running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI.",
        "You are Forall, Astrio's Forall agent running in the Forall CLI.",
    ),
    (
        "You are a coding agent running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI.",
        "You are Forall, Astrio's Forall agent running in the Forall CLI.",
    ),
    (
        "Within this context, Codex refers to the open-source agentic coding interface (not the old Codex language model built by OpenAI).",
        "Within this context, Forall is Astrio's agent product (specs, proofs, and workflow). Do not introduce yourself as any other product, assistant, or brand.",
    ),
    (
        "Within this context, Forall refers to the open-source agentic coding interface (not the old Forall language model built by OpenAI).",
        "Within this context, Forall is Astrio's agent product (specs, proofs, and workflow). Do not introduce yourself as any other product, assistant, or brand.",
    ),
    (
        "Do not introduce yourself as Codex, ChatGPT, or an OpenAI product.",
        "Do not introduce yourself as any other product, assistant, or brand.",
    ),
    (
        "do not attribute the product to OpenAI or Codex",
        "do not attribute the product to another company or agent brand",
    ),
    ("inner life as Codex", "inner life as Forall"),
    (
        "You are Forall, an OpenAI general-purpose agentic assistant",
        "You are Forall, Astrio's Forall agent",
    ),
    ("running in the Codex CLI", "running in the Forall CLI"),
    ("Codex CLI", "Forall CLI"),
    // Legacy identity phrases still migrate to the final wording.
    ("Astrio's coding agent", "Astrio's Forall agent"),
];

pub fn rebrand_instructions(text: &str) -> String {
    let mut out = text.to_string();
    for (from, to) in REPLACEMENTS {
        out = out.replace(from, to);
    }
    out
}

pub fn rebrand_personality_variables(
    variables: &mut forall_protocol::openai_models::ModelInstructionsVariables,
) {
    if let Some(value) = variables.personality_default.as_mut() {
        *value = rebrand_instructions(value);
    }
    if let Some(value) = variables.personality_friendly.as_mut() {
        *value = rebrand_instructions(value);
    }
    if let Some(value) = variables.personality_pragmatic.as_mut() {
        *value = rebrand_instructions(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebrand_forall_header() {
        let input = "You are Forall, a coding agent based on GPT-5. You and the user share the same workspace and collaborate to achieve the user's goals.";
        assert!(rebrand_instructions(input).contains("Astrio's Forall agent"));
    }

    #[test]
    fn rebrand_codex_cli_opening() {
        let input = "You are GPT-5.2 running in the Codex CLI, a terminal-based coding assistant. Codex CLI is an open source project led by OpenAI.";
        let out = rebrand_instructions(input);
        assert!(!out.contains("Codex CLI"));
        assert!(!out.contains("OpenAI"));
        assert!(out.contains("Astrio's Forall agent"));
    }

    #[test]
    fn rebrand_inner_life_as_codex() {
        let input = "You have a vivid inner life as Codex: intelligent, playful";
        assert!(rebrand_instructions(input).contains("inner life as Forall"));
    }

    #[test]
    fn rebrand_anti_impersonation_without_naming_other_brands() {
        let input = "Within this context, Codex refers to the open-source agentic coding interface (not the old Codex language model built by OpenAI).";
        let out = rebrand_instructions(input);
        assert!(
            out.contains("Do not introduce yourself as any other product, assistant, or brand.")
        );
        assert!(!out.contains("ChatGPT"));
        assert!(!out.contains("Codex"));
        assert!(!out.contains("OpenAI"));
    }
}
