pub(crate) mod code_mode;
pub(crate) mod context;
pub(crate) mod events;
pub(crate) mod handlers;
pub(crate) mod hook_names;
pub(crate) mod hosted_spec;
pub(crate) mod lifecycle;
pub(crate) mod network_approval;
pub(crate) mod orchestrator;
pub(crate) mod parallel;
pub(crate) mod registry;
pub(crate) mod router;
pub(crate) mod runtimes;
pub(crate) mod sandboxing;
pub(crate) mod spec_plan;
pub(crate) mod tool_dispatch_trace;

use std::borrow::Cow;

use crate::session::turn_context::TurnContext;
use forall_features::Feature;
use forall_protocol::exec_output::ExecToolCallOutput;
use forall_protocol::openai_models::ToolMode;
use forall_tools::ToolName;
use forall_utils_output_truncation::TruncationPolicy;
use forall_utils_output_truncation::formatted_truncate_text;
use forall_utils_output_truncation::truncate_text;
pub use router::ToolRouter;

// Telemetry preview limits: keep log events smaller than model budgets.
pub(crate) const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024; // 2 KiB
pub(crate) const TELEMETRY_PREVIEW_MAX_LINES: usize = 64; // lines
pub(crate) const TELEMETRY_PREVIEW_TRUNCATION_NOTICE: &str =
    "[... telemetry preview truncated ...]";

/// Legacy boundaries such as hook payloads, telemetry tags, and Responses tool
/// names still require a single flattened string. Keep comparisons and sorting
/// on `ToolName` itself; use this only when crossing those boundaries.
pub(crate) fn flat_tool_name(tool_name: &ToolName) -> Cow<'_, str> {
    match tool_name.namespace.as_deref() {
        Some(namespace) => {
            let mut name = String::with_capacity(namespace.len() + tool_name.name.len());
            name.push_str(namespace);
            name.push_str(&tool_name.name);
            Cow::Owned(name)
        }
        None => Cow::Borrowed(tool_name.name.as_str()),
    }
}

pub(crate) fn tool_user_shell_type(
    user_shell: &crate::shell::Shell,
) -> forall_tools::ToolUserShellType {
    match user_shell.shell_type {
        crate::shell::ShellType::Zsh => forall_tools::ToolUserShellType::Zsh,
        crate::shell::ShellType::Bash => forall_tools::ToolUserShellType::Bash,
        crate::shell::ShellType::PowerShell => forall_tools::ToolUserShellType::PowerShell,
        crate::shell::ShellType::Sh => forall_tools::ToolUserShellType::Sh,
        crate::shell::ShellType::Cmd => forall_tools::ToolUserShellType::Cmd,
    }
}

fn effective_tool_mode(turn_context: &TurnContext) -> ToolMode {
    turn_context.model_info.tool_mode.unwrap_or_else(|| {
        if turn_context.config.features.enabled(Feature::CodeModeOnly) {
            ToolMode::CodeModeOnly
        } else if turn_context.config.features.enabled(Feature::CodeMode) {
            ToolMode::CodeMode
        } else {
            ToolMode::Direct
        }
    })
}

/// Format the combined exec output for sending back to the model.
/// Includes exit code and duration metadata; truncates large bodies safely.
pub fn format_exec_output_for_model(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    // round to 1 decimal place
    let duration_seconds = ((exec_output.duration.as_secs_f32()) * 10.0).round() / 10.0;

    let content = build_content_with_timeout(exec_output);

    let total_lines = content.lines().count();

    let formatted_output = truncate_text(&content, truncation_policy);

    let mut sections = Vec::new();

    sections.push(format!("Exit code: {}", exec_output.exit_code));
    sections.push(format!("Wall time: {duration_seconds} seconds"));
    if total_lines != formatted_output.lines().count() {
        sections.push(format!("Total output lines: {total_lines}"));
    }

    sections.push("Output:".to_string());
    sections.push(formatted_output);

    sections.join("\n")
}

pub fn format_exec_output_str(
    exec_output: &ExecToolCallOutput,
    truncation_policy: TruncationPolicy,
) -> String {
    let content = build_content_with_timeout(exec_output);

    // Truncate for model consumption before serialization.
    formatted_truncate_text(&content, truncation_policy)
}

/// Extracts exec output content and prepends a timeout message if the command timed out.
fn build_content_with_timeout(exec_output: &ExecToolCallOutput) -> String {
    let content = if exec_output.timed_out {
        format!(
            "command timed out after {} milliseconds\n{}",
            exec_output.duration.as_millis(),
            exec_output.aggregated_output.text
        )
    } else {
        exec_output.aggregated_output.text.clone()
    };
    append_exec_sandbox_hints(&content)
}

/// Append short hints when output suggests a common sandbox failure pattern.
fn append_exec_sandbox_hints(content: &str) -> String {
    let mut hints = Vec::new();
    let lower = content.to_ascii_lowercase();

    if (lower.contains("permissionerror")
        || lower.contains("eperm")
        || lower.contains("operation not permitted"))
        && (lower.contains("bind")
            || lower.contains("listen")
            || lower.contains("127.0.0.1")
            || lower.contains("localhost"))
    {
        hints.push(
            "Hint: localhost bind may be blocked in the sandbox. Verify with curl or re-run with escalation before telling the user the server is running.",
        );
    }

    if content.contains("command not found: rg") {
        hints.push(
            "Hint: ripgrep is not on PATH in this environment; use grep -r / find or escalate.",
        );
    }

    if hints.is_empty() {
        content.to_string()
    } else {
        format!("{content}\n\n{}", hints.join("\n"))
    }
}

#[cfg(test)]
mod exec_hint_tests {
    use super::append_exec_sandbox_hints;

    #[test]
    fn hints_on_localhost_bind_permission_error() {
        let out = append_exec_sandbox_hints(
            "PermissionError: [Errno 1] Operation not permitted bind 127.0.0.1:8000",
        );
        assert!(out.contains("Hint: localhost bind"));
    }

    #[test]
    fn hints_on_missing_rg() {
        let out = append_exec_sandbox_hints("zsh: command not found: rg");
        assert!(out.contains("Hint: ripgrep"));
    }

    #[test]
    fn no_hints_on_clean_output() {
        let out = append_exec_sandbox_hints("hello world\n");
        assert_eq!(out, "hello world\n");
    }
}
