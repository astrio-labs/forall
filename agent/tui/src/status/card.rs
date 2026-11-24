use crate::history_cell::CompositeHistoryCell;
use crate::history_cell::HistoryCell;
use crate::history_cell::PlainHistoryCell;
use crate::history_cell::plain_lines;
use crate::history_cell::with_border_with_inner_width;
use crate::legacy_core::config::Config;
use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;
use crate::version::FORALL_CLI_VERSION;
use forall_app_server_protocol::AskForApproval;
use forall_model_provider_info::WireApi;
use forall_protocol::ThreadId;
use forall_protocol::config_types::ApprovalsReviewer;
use forall_protocol::models::ActivePermissionProfile;
use forall_protocol::models::BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS;
use forall_protocol::models::BUILT_IN_PERMISSION_PROFILE_READ_ONLY;
use forall_protocol::models::BUILT_IN_PERMISSION_PROFILE_WORKSPACE;
use forall_protocol::models::PermissionProfile;
use forall_protocol::openai_models::ReasoningEffort;
use forall_utils_absolute_path::AbsolutePathBuf;
use forall_utils_sandbox_summary::summarize_permission_profile;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::collections::BTreeSet;
use std::path::PathBuf;
use url::Url;

use super::account::StatusAccountDisplay;
use super::format::FieldFormatter;
use super::format::line_display_width;
use super::format::push_label;
use super::format::truncate_line_to_width;
use super::helpers::compose_account_display;
use super::helpers::compose_model_display;
use super::helpers::format_directory_display;
use super::helpers::format_tokens_compact;
use super::remote_connection::RemoteConnectionStatus;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;

#[derive(Debug, Clone)]
struct StatusContextWindowData {
    percent_remaining: i64,
    tokens_in_context: i64,
    window: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct StatusTokenUsageData {
    total: i64,
    input: i64,
    output: i64,
    context_window: Option<StatusContextWindowData>,
}

#[derive(Debug)]
struct StatusHistoryCell {
    model_name: String,
    model_details: Vec<String>,
    directory: PathBuf,
    permissions: String,
    agents_summary: String,
    collaboration_mode: Option<String>,
    model_provider: Option<String>,
    remote_connection: Option<RemoteConnectionStatus>,
    account: Option<StatusAccountDisplay>,
    thread_name: Option<String>,
    session_id: Option<String>,
    forked_from: Option<String>,
    token_usage: StatusTokenUsageData,
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output(
    config: &Config,
    account_display: Option<&StatusAccountDisplay>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    forked_from: Option<ThreadId>,
    model_name: &str,
    collaboration_mode: Option<&str>,
    reasoning_effort_override: Option<Option<ReasoningEffort>>,
) -> CompositeHistoryCell {
    new_status_output_handle(
        config,
        /*runtime_model_provider_base_url*/ None,
        /*remote_connection*/ None,
        account_display,
        token_info,
        total_usage,
        session_id,
        thread_name,
        forked_from,
        model_name,
        collaboration_mode,
        reasoning_effort_override,
        "<none>".to_string(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn new_status_output_handle(
    config: &Config,
    runtime_model_provider_base_url: Option<&str>,
    remote_connection: Option<&RemoteConnectionStatus>,
    account_display: Option<&StatusAccountDisplay>,
    token_info: Option<&TokenUsageInfo>,
    total_usage: &TokenUsage,
    session_id: &Option<ThreadId>,
    thread_name: Option<String>,
    forked_from: Option<ThreadId>,
    model_name: &str,
    collaboration_mode: Option<&str>,
    reasoning_effort_override: Option<Option<ReasoningEffort>>,
    agents_summary: String,
) -> CompositeHistoryCell {
    let command = PlainHistoryCell::new(vec!["/status".magenta().into()]);
    let card = StatusHistoryCell::new(
        config,
        runtime_model_provider_base_url,
        remote_connection,
        account_display,
        token_info,
        total_usage,
        session_id,
        thread_name,
        forked_from,
        model_name,
        collaboration_mode,
        reasoning_effort_override,
        agents_summary,
    );

    CompositeHistoryCell::new(vec![Box::new(command), Box::new(card)])
}

impl StatusHistoryCell {
    #[allow(clippy::too_many_arguments)]
    fn new(
        config: &Config,
        runtime_model_provider_base_url: Option<&str>,
        remote_connection: Option<&RemoteConnectionStatus>,
        account_display: Option<&StatusAccountDisplay>,
        token_info: Option<&TokenUsageInfo>,
        total_usage: &TokenUsage,
        session_id: &Option<ThreadId>,
        thread_name: Option<String>,
        forked_from: Option<ThreadId>,
        model_name: &str,
        collaboration_mode: Option<&str>,
        reasoning_effort_override: Option<Option<ReasoningEffort>>,
        agents_summary: String,
    ) -> Self {
        let approval_policy = AskForApproval::from(config.permissions.approval_policy.value());
        let permission_profile = config.permissions.effective_permission_profile();
        let workspace_roots = config.effective_workspace_roots();
        let mut config_entries = vec![
            ("workdir", config.cwd.display().to_string()),
            ("model", model_name.to_string()),
            ("provider", config.model_provider_id.clone()),
            (
                "approval",
                config.permissions.approval_policy.value().to_string(),
            ),
            (
                "sandbox",
                summarize_permission_profile(
                    &permission_profile,
                    &config.cwd,
                    workspace_roots.as_slice(),
                ),
            ),
        ];
        if matches!(
            config.model_provider.wire_api,
            WireApi::Responses | WireApi::Messages | WireApi::Gemini | WireApi::BedrockMessages
        ) {
            let effort_value = reasoning_effort_override
                .unwrap_or_else(|| config.model_reasoning_effort.clone())
                .map(|effort| effort.to_string())
                .unwrap_or_else(|| "none".to_string());
            config_entries.push(("reasoning effort", effort_value));
            config_entries.push((
                "reasoning summaries",
                config
                    .model_reasoning_summary
                    .map(|summary| summary.to_string())
                    .unwrap_or_else(|| "auto".to_string()),
            ));
        }
        let (model_name, model_details) = compose_model_display(model_name, &config_entries);
        let approval = config_entries
            .iter()
            .find(|(k, _)| *k == "approval")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "<unknown>".to_string());
        let active_permission_profile = config.permissions.active_permission_profile();
        let sandbox =
            status_permission_summary(&permission_profile, &config.cwd, workspace_roots.as_slice());
        let workspace_root_suffix = workspace_root_suffix(workspace_roots.as_slice(), &config.cwd);
        let approval = status_approval_label(approval_policy, config.approvals_reviewer, &approval);
        let permissions = status_permissions_label(
            active_permission_profile.as_ref(),
            &permission_profile,
            approval_policy,
            &sandbox,
            &approval,
            workspace_root_suffix.as_deref(),
        );
        let model_provider = format_model_provider(config, runtime_model_provider_base_url);
        let account = compose_account_display(account_display);
        let session_id = session_id.as_ref().map(std::string::ToString::to_string);
        let forked_from = forked_from.map(|id| id.to_string());
        let default_usage = TokenUsage::default();
        let (context_usage, context_window) = match token_info {
            Some(info) => (&info.last_token_usage, info.model_context_window),
            None => (&default_usage, config.model_context_window),
        };
        let context_window = context_window.map(|window| StatusContextWindowData {
            percent_remaining: context_usage.percent_of_context_window_remaining(window),
            tokens_in_context: context_usage.tokens_in_context_window(),
            window,
        });

        let token_usage = StatusTokenUsageData {
            total: total_usage.blended_total(),
            input: total_usage.non_cached_input(),
            output: total_usage.output_tokens,
            context_window,
        };

        Self {
            model_name,
            model_details,
            directory: config.cwd.to_path_buf(),
            permissions,
            collaboration_mode: collaboration_mode.map(ToString::to_string),
            model_provider,
            remote_connection: remote_connection.cloned(),
            account,
            thread_name,
            session_id,
            forked_from,
            token_usage,
            agents_summary,
        }
    }

    fn token_usage_spans(&self) -> Vec<Span<'static>> {
        let total_fmt = format_tokens_compact(self.token_usage.total);
        let input_fmt = format_tokens_compact(self.token_usage.input);
        let output_fmt = format_tokens_compact(self.token_usage.output);

        vec![
            Span::from(total_fmt),
            Span::from(" total "),
            Span::from(" (").dim(),
            Span::from(input_fmt).dim(),
            Span::from(" input").dim(),
            Span::from(" + ").dim(),
            Span::from(output_fmt).dim(),
            Span::from(" output").dim(),
            Span::from(")").dim(),
        ]
    }

    fn context_window_spans(&self) -> Option<Vec<Span<'static>>> {
        let context = self.token_usage.context_window.as_ref()?;
        let percent = context.percent_remaining;
        let used_fmt = format_tokens_compact(context.tokens_in_context);
        let window_fmt = format_tokens_compact(context.window);

        Some(vec![
            Span::from(format!("{percent}% left")),
            Span::from(" (").dim(),
            Span::from(used_fmt).dim(),
            Span::from(" used / ").dim(),
            Span::from(window_fmt).dim(),
            Span::from(")").dim(),
        ])
    }
}

fn status_permission_summary(
    permission_profile: &PermissionProfile,
    cwd: &AbsolutePathBuf,
    workspace_roots: &[AbsolutePathBuf],
) -> String {
    let summary = summarize_permission_profile(permission_profile, cwd, workspace_roots);
    if let Some(details) = summary.strip_prefix("read-only") {
        if details.contains("(network access enabled)") {
            return "read-only with network access".to_string();
        }
        return "read-only".to_string();
    }
    if let Some(details) = summary.strip_prefix("workspace-write") {
        if details.contains("(network access enabled)") {
            return "workspace with network access".to_string();
        }
        return "workspace".to_string();
    }
    if summary == "custom permissions (network access enabled)" {
        return "custom permissions with network access".to_string();
    }
    summary
}

fn workspace_root_suffix(
    workspace_roots: &[AbsolutePathBuf],
    cwd: &AbsolutePathBuf,
) -> Option<String> {
    let extra_roots = workspace_roots
        .iter()
        .filter(|root| *root != cwd)
        .map(|root| root.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if extra_roots.is_empty() {
        None
    } else {
        Some(format!(" [{}]", extra_roots.join(", ")))
    }
}

fn status_permissions_label(
    active_permission_profile: Option<&ActivePermissionProfile>,
    permission_profile: &PermissionProfile,
    approval_policy: AskForApproval,
    sandbox: &str,
    approval: &str,
    workspace_root_suffix: Option<&str>,
) -> String {
    let active_id = active_permission_profile.map(|active| active.id.as_str());
    match active_id {
        Some(BUILT_IN_PERMISSION_PROFILE_READ_ONLY) => {
            let label = if sandbox == "read-only with network access" {
                "Read Only with network access"
            } else {
                "Read Only"
            };
            return format!("{label} ({approval})");
        }
        Some(BUILT_IN_PERMISSION_PROFILE_WORKSPACE) => match sandbox {
            "workspace" => {
                return format!(
                    "Workspace{} ({approval})",
                    workspace_root_suffix.unwrap_or("")
                );
            }
            "workspace with network access" => {
                return format!(
                    "Workspace with network access{} ({approval})",
                    workspace_root_suffix.unwrap_or("")
                );
            }
            _ => {}
        },
        Some(BUILT_IN_PERMISSION_PROFILE_DANGER_FULL_ACCESS)
            if permission_profile == &PermissionProfile::Disabled =>
        {
            return if approval_policy == AskForApproval::Never {
                "Full Access".to_string()
            } else {
                format!("No Sandbox ({approval})")
            };
        }
        Some(id) => {
            let sandbox = decorate_workspace_sandbox_label(sandbox, workspace_root_suffix);
            return format!("Profile {id} ({sandbox}, {approval})");
        }
        None => {}
    }

    if sandbox == "read-only" {
        return format!("Read Only ({approval})");
    }
    if approval_policy == AskForApproval::OnRequest && sandbox == "workspace" {
        return format!(
            "Workspace{} ({approval})",
            workspace_root_suffix.unwrap_or("")
        );
    }
    if approval_policy == AskForApproval::Never
        && permission_profile == &PermissionProfile::Disabled
    {
        return "Full Access".to_string();
    }
    let sandbox = decorate_workspace_sandbox_label(sandbox, workspace_root_suffix);
    format!("Custom ({sandbox}, {approval})")
}

fn decorate_workspace_sandbox_label(sandbox: &str, workspace_root_suffix: Option<&str>) -> String {
    match workspace_root_suffix {
        Some(suffix) if sandbox.starts_with("workspace") => format!("{sandbox}{suffix}"),
        _ => sandbox.to_string(),
    }
}

fn status_approval_label(
    approval_policy: AskForApproval,
    approvals_reviewer: ApprovalsReviewer,
    approval: &str,
) -> String {
    if approval_policy == AskForApproval::OnRequest {
        return match approvals_reviewer {
            ApprovalsReviewer::AutoReview => "Approve for me".to_string(),
            ApprovalsReviewer::User => "Ask for approval".to_string(),
        };
    }

    approval.to_string()
}

impl HistoryCell for StatusHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![
            Span::from(format!("{}>_ ", FieldFormatter::INDENT)).dim(),
            Span::from(crate::branding::PRODUCT_FULL_NAME).bold(),
            Span::from(" ").dim(),
            Span::from(format!("(v{FORALL_CLI_VERSION})")).dim(),
        ]));

        let available_inner_width = usize::from(width.saturating_sub(4));
        if available_inner_width == 0 {
            return Vec::new();
        }

        let account_value = self.account.as_ref().map(|account| match account {
            StatusAccountDisplay::ApiKey => "API key configured".to_string(),
        });

        let mut labels: Vec<String> = vec!["Model", "Directory", "Permissions", "Agents.md"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut seen: BTreeSet<String> = labels.iter().cloned().collect();
        let thread_name = self.thread_name.as_deref().filter(|name| !name.is_empty());

        if self.model_provider.is_some() {
            push_label(&mut labels, &mut seen, "Model provider");
        }
        if account_value.is_some() {
            push_label(&mut labels, &mut seen, "Account");
        }
        if thread_name.is_some() {
            push_label(&mut labels, &mut seen, "Thread name");
        }
        if self.session_id.is_some() {
            push_label(&mut labels, &mut seen, "Session");
        }
        if self.session_id.is_some() && self.forked_from.is_some() {
            push_label(&mut labels, &mut seen, "Forked from");
        }
        if self.collaboration_mode.is_some() {
            push_label(&mut labels, &mut seen, "Collaboration mode");
        }
        push_label(&mut labels, &mut seen, "Token usage");
        if self.token_usage.context_window.is_some() {
            push_label(&mut labels, &mut seen, "Context window");
        }

        let formatter = FieldFormatter::from_labels(labels.iter().map(String::as_str));
        let value_width = formatter.value_width(available_inner_width);

        if let Some(remote_connection) = self.remote_connection.as_ref() {
            let wrapped_remote = word_wrap_lines(
                [Line::from(vec![
                    Span::from(remote_connection.address.clone()),
                    Span::from(" (").dim(),
                    Span::from(remote_connection.version.clone()).dim(),
                    Span::from(")").dim(),
                ])],
                RtOptions::new(value_width.max(1)),
            );
            let mut wrapped_remote = wrapped_remote.into_iter();
            if let Some(first) = wrapped_remote.next() {
                lines.push(formatter.line("Remote", first.spans));
                lines.extend(wrapped_remote.map(|line| formatter.continuation(line.spans)));
            }
            lines.push(Line::from(Vec::<Span<'static>>::new()));
        }

        let mut model_spans = vec![Span::from(self.model_name.clone())];
        if !self.model_details.is_empty() {
            model_spans.push(Span::from(" (").dim());
            model_spans.push(Span::from(self.model_details.join(", ")).dim());
            model_spans.push(Span::from(")").dim());
        }

        let directory_value = format_directory_display(&self.directory, Some(value_width));

        lines.push(formatter.line("Model", model_spans));
        if let Some(model_provider) = self.model_provider.as_ref() {
            lines.push(formatter.line("Model provider", vec![Span::from(model_provider.clone())]));
        }
        lines.push(formatter.line("Directory", vec![Span::from(directory_value)]));
        lines.push(formatter.line("Permissions", vec![Span::from(self.permissions.clone())]));
        lines.push(formatter.line("Agents.md", vec![Span::from(self.agents_summary.clone())]));

        if let Some(account_value) = account_value {
            lines.push(formatter.line("Account", vec![Span::from(account_value)]));
        }

        if let Some(thread_name) = thread_name {
            lines.push(formatter.line("Thread name", vec![Span::from(thread_name.to_string())]));
        }
        if let Some(collab_mode) = self.collaboration_mode.as_ref() {
            lines.push(formatter.line("Collaboration mode", vec![Span::from(collab_mode.clone())]));
        }
        if let Some(session) = self.session_id.as_ref() {
            lines.push(formatter.line("Session", vec![Span::from(session.clone())]));
        }
        if self.session_id.is_some()
            && let Some(forked_from) = self.forked_from.as_ref()
        {
            lines.push(formatter.line("Forked from", vec![Span::from(forked_from.clone())]));
        }

        lines.push(Line::from(Vec::<Span<'static>>::new()));
        lines.push(formatter.line("Token usage", self.token_usage_spans()));

        if let Some(spans) = self.context_window_spans() {
            lines.push(formatter.line("Context window", spans));
        }

        let content_width = lines.iter().map(line_display_width).max().unwrap_or(0);
        let inner_width = content_width.min(available_inner_width);
        let truncated_lines: Vec<Line<'static>> = lines
            .into_iter()
            .map(|line| truncate_line_to_width(line, inner_width))
            .collect();

        with_border_with_inner_width(truncated_lines, inner_width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(self.display_lines(u16::MAX))
    }

    fn display_hyperlink_lines(
        &self,
        width: u16,
    ) -> Vec<crate::terminal_hyperlinks::HyperlinkLine> {
        crate::terminal_hyperlinks::plain_hyperlink_lines(self.display_lines(width))
    }

    fn transcript_hyperlink_lines(
        &self,
        width: u16,
    ) -> Vec<crate::terminal_hyperlinks::HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }
}

fn format_model_provider(config: &Config, runtime_base_url: Option<&str>) -> Option<String> {
    let provider = &config.model_provider;
    let name = provider.name.trim();
    let provider_name = if name.is_empty() {
        config.model_provider_id.as_str()
    } else {
        name
    };
    let base_url = runtime_base_url.and_then(sanitize_base_url);
    let is_default_openai = provider.is_openai() && base_url.is_none();
    if is_default_openai {
        return None;
    }

    Some(match base_url {
        Some(base_url) => format!("{provider_name} - {base_url}"),
        None => provider_name.to_string(),
    })
}

fn sanitize_base_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let Ok(mut url) = Url::parse(trimmed) else {
        return None;
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string().trim_end_matches('/').to_string()).filter(|value| !value.is_empty())
}
