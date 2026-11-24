//! Session headers, onboarding guidance, and transcript cards.

use super::*;
use crate::style::AccentStylize;

pub(crate) const SESSION_HEADER_MAX_INNER_WIDTH: usize = 56; // Just an eyeballed value

const SESSION_METADATA_VERSION_LABEL: &str = "version:";
const SESSION_METADATA_MODEL_LABEL: &str = "model:";
const SESSION_METADATA_DIR_LABEL: &str = "directory:";
const SESSION_METADATA_PERMISSIONS_LABEL: &str = "permissions:";
const SESSION_METADATA_TIP_LABEL: &str = "tip:";

pub(crate) fn session_metadata_label_width(yolo_mode: bool) -> usize {
    let width = SESSION_METADATA_VERSION_LABEL
        .len()
        .max(SESSION_METADATA_MODEL_LABEL.len())
        .max(SESSION_METADATA_DIR_LABEL.len())
        .max(SESSION_METADATA_TIP_LABEL.len());
    if yolo_mode {
        width.max(SESSION_METADATA_PERMISSIONS_LABEL.len())
    } else {
        width
    }
}

fn session_metadata_label_prefix(label: &str, label_width: usize) -> String {
    format!("{label:<label_width$} ")
}

/// Render `lines` inside a border whose inner width is at least `inner_width`.
///
/// This is useful when callers have already clamped their content to a
/// specific width and want the border math centralized here instead of
/// duplicating padding logic in the TUI widgets themselves.
pub(crate) fn with_border_with_inner_width(
    lines: Vec<Line<'static>>,
    inner_width: usize,
) -> Vec<Line<'static>> {
    with_border_internal(lines, Some(inner_width))
}

fn with_border_internal(
    lines: Vec<Line<'static>>,
    forced_inner_width: Option<usize>,
) -> Vec<Line<'static>> {
    let max_line_width = lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
                .sum::<usize>()
        })
        .max()
        .unwrap_or(0);
    let content_width = forced_inner_width
        .unwrap_or(max_line_width)
        .max(max_line_width);

    let mut out = Vec::with_capacity(lines.len() + 2);
    let border_inner_width = content_width + 2;
    out.push(vec![format!("╭{}╮", "─".repeat(border_inner_width)).dim()].into());

    for line in lines.into_iter() {
        let used_width: usize = line
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        let span_count = line.spans.len();
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(span_count + 4);
        spans.push(Span::from("│ ").dim());
        spans.extend(line);
        if used_width < content_width {
            spans.push(Span::from(" ".repeat(content_width - used_width)).dim());
        }
        spans.push(Span::from(" │").dim());
        out.push(Line::from(spans));
    }

    out.push(vec![format!("╰{}╯", "─".repeat(border_inner_width)).dim()].into());

    out
}

#[derive(Debug)]
struct TooltipHistoryCell {
    tip: String,
    cwd: PathBuf,
    label_width: usize,
}

impl TooltipHistoryCell {
    fn new(tip: String, cwd: &Path, label_width: usize) -> Self {
        Self {
            tip,
            cwd: cwd.to_path_buf(),
            label_width,
        }
    }
}

impl HistoryCell for TooltipHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let label_prefix =
            session_metadata_label_prefix(SESSION_METADATA_TIP_LABEL, self.label_width);
        let prefix_width = UnicodeWidthStr::width(label_prefix.as_str());
        let wrap_width = usize::from(width.max(1))
            .saturating_sub(prefix_width)
            .max(1);
        let mut lines: Vec<Line<'static>> = Vec::new();
        append_markdown(
            &self.tip,
            Some(wrap_width),
            Some(self.cwd.as_path()),
            &mut lines,
        );

        prefix_lines(
            lines,
            Span::from(label_prefix).dim(),
            Span::from(" ".repeat(prefix_width)),
        )
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        vec![Line::from(format!("Tip: {}", self.tip))]
    }
}

#[derive(Debug)]
pub struct SessionInfoCell(CompositeHistoryCell);

impl HistoryCell for SessionInfoCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.0.display_lines(width)
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.0.desired_height(width)
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.0.transcript_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        self.0.raw_lines()
    }
}

pub(crate) fn new_session_info(
    config: &Config,
    requested_model: &str,
    session: &ThreadSessionState,
    is_first_event: bool,
    tooltip_override: Option<String>,
    auth_plan: Option<PlanType>,
    show_fast_status: bool,
) -> SessionInfoCell {
    let yolo_mode = has_yolo_permissions(session.approval_policy, &session.permission_profile);
    let metadata_label_width = session_metadata_label_width(yolo_mode);
    // Header box rendered as history (so it appears at the very top)
    let header = SessionHeaderHistoryCell::new(
        session.model.clone(),
        session.reasoning_effort.clone(),
        show_fast_status,
        config.cwd.to_path_buf(),
        FORALL_CLI_VERSION,
    )
    .with_yolo_mode(yolo_mode);
    let mut parts: Vec<Box<dyn HistoryCell>> = vec![Box::new(header)];

    if is_first_event {
        // Help lines below the header (new copy and list)
        let help_lines: Vec<Line<'static>> = vec![
            "  To get started, describe a task or try one of these commands:"
                .dim()
                .into(),
            Line::from(""),
            Line::from(vec![
                "  ".into(),
                "/init".into(),
                " - create an AGENTS.md file with instructions for Forall".dim(),
            ]),
            Line::from(vec![
                "  ".into(),
                "/status".into(),
                " - show current session configuration".dim(),
            ]),
            Line::from(vec![
                "  ".into(),
                "/permissions".into(),
                " - choose what Forall is allowed to do".dim(),
            ]),
            Line::from(vec![
                "  ".into(),
                "/model".into(),
                " - choose what model and reasoning effort to use".dim(),
            ]),
            Line::from(vec![
                "  ".into(),
                "/review".into(),
                " - review any changes and find issues".dim(),
            ]),
        ];

        parts.push(Box::new(PlainHistoryCell { lines: help_lines }));
    } else {
        if config.show_tooltips
            && let Some(tooltips) = tooltip_override
                .or_else(|| tooltips::get_tooltip(auth_plan, show_fast_status))
                .map(|tip| TooltipHistoryCell::new(tip, &config.cwd, metadata_label_width))
        {
            parts.push(Box::new(tooltips));
        }
        if requested_model != session.model.as_str() {
            let lines = vec![
                "model changed:".magenta().bold().into(),
                format!("requested: {requested_model}").into(),
                format!("used: {}", session.model).into(),
            ];
            parts.push(Box::new(PlainHistoryCell { lines }));
        }
    }

    SessionInfoCell(CompositeHistoryCell { parts })
}

pub(crate) fn is_yolo_mode(config: &Config) -> bool {
    has_yolo_permissions(
        AskForApproval::from(config.permissions.approval_policy.value()),
        &config.permissions.effective_permission_profile(),
    )
}

pub(crate) fn has_yolo_permissions(
    approval_policy: AskForApproval,
    permission_profile: &PermissionProfile,
) -> bool {
    approval_policy == AskForApproval::Never
        && matches!(
            permission_profile,
            PermissionProfile::Disabled
                | PermissionProfile::Managed {
                    file_system: ManagedFileSystemPermissions::Unrestricted,
                    network: NetworkSandboxPolicy::Enabled,
                }
        )
}
#[derive(Debug)]
pub(crate) struct SessionHeaderHistoryCell {
    version: &'static str,
    model: String,
    model_style: Style,
    reasoning_effort: Option<ReasoningEffortConfig>,
    show_fast_status: bool,
    directory: PathBuf,
    yolo_mode: bool,
}

impl SessionHeaderHistoryCell {
    pub(crate) fn new(
        model: String,
        reasoning_effort: Option<ReasoningEffortConfig>,
        show_fast_status: bool,
        directory: PathBuf,
        version: &'static str,
    ) -> Self {
        Self::new_with_style(
            model,
            Style::default(),
            reasoning_effort,
            show_fast_status,
            directory,
            version,
        )
    }

    pub(crate) fn new_with_style(
        model: String,
        model_style: Style,
        reasoning_effort: Option<ReasoningEffortConfig>,
        show_fast_status: bool,
        directory: PathBuf,
        version: &'static str,
    ) -> Self {
        Self {
            version,
            model,
            model_style,
            reasoning_effort,
            show_fast_status,
            directory,
            yolo_mode: false,
        }
    }

    pub(crate) fn with_yolo_mode(mut self, yolo_mode: bool) -> Self {
        self.yolo_mode = yolo_mode;
        self
    }

    fn format_directory(&self, max_width: Option<usize>) -> String {
        Self::format_directory_inner(&self.directory, max_width)
    }

    pub(crate) fn format_directory_inner(directory: &Path, max_width: Option<usize>) -> String {
        let formatted = if let Some(rel) = relativize_to_home(directory) {
            if rel.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display())
            }
        } else {
            directory.display().to_string()
        };

        if let Some(max_width) = max_width {
            if max_width == 0 {
                return String::new();
            }
            if UnicodeWidthStr::width(formatted.as_str()) > max_width {
                return crate::text_formatting::center_truncate_path(&formatted, max_width);
            }
        }

        formatted
    }

    fn reasoning_label(&self) -> Option<&str> {
        self.reasoning_effort
            .as_ref()
            .map(ReasoningEffortConfig::as_str)
    }
}

impl HistoryCell for SessionHeaderHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if width == 0 {
            return Vec::new();
        }

        let content_width = std::cmp::min(width as usize, SESSION_HEADER_MAX_INNER_WIDTH);

        let make_row = |spans: Vec<Span<'static>>| Line::from(spans);

        const CHANGE_MODEL_HINT_COMMAND: &str = "/model";
        const CHANGE_MODEL_HINT_EXPLANATION: &str = " to change";
        let label_width = session_metadata_label_width(self.yolo_mode);

        let mut lines: Vec<Line<'static>> = crate::branding::SESSION_HEADER_ASCII_ART
            .trim()
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect();

        if !lines.is_empty() {
            lines.push(make_row(Vec::new()));
        }

        lines.push(make_row(vec![
            Span::from(session_metadata_label_prefix(
                SESSION_METADATA_VERSION_LABEL,
                label_width,
            ))
            .dim(),
            Span::from(format!("v{}", self.version)),
        ]));

        let model_label = session_metadata_label_prefix(SESSION_METADATA_MODEL_LABEL, label_width);
        let reasoning_label = self.reasoning_label();
        let model_spans: Vec<Span<'static>> = {
            let mut spans = vec![
                Span::from(model_label).dim(),
                Span::styled(self.model.clone(), self.model_style),
            ];
            if let Some(reasoning) = reasoning_label {
                spans.push(Span::from(" "));
                spans.push(Span::from(reasoning.to_owned()));
            }
            if self.show_fast_status {
                spans.push("   ".into());
                spans.push(Span::styled("fast", self.model_style.magenta()));
            }
            spans.push("   ".dim());
            spans.push(CHANGE_MODEL_HINT_COMMAND.accent());
            spans.push(CHANGE_MODEL_HINT_EXPLANATION.dim());
            spans
        };

        let dir_prefix = session_metadata_label_prefix(SESSION_METADATA_DIR_LABEL, label_width);
        let dir_prefix_width = UnicodeWidthStr::width(dir_prefix.as_str());
        let dir_max_width = content_width.saturating_sub(dir_prefix_width);
        let dir = self.format_directory(Some(dir_max_width));
        let dir_spans = vec![Span::from(dir_prefix).dim(), Span::from(dir)];

        lines.push(make_row(model_spans));
        lines.push(make_row(dir_spans));

        if self.yolo_mode {
            lines.push(make_row(vec![
                Span::from(session_metadata_label_prefix(
                    SESSION_METADATA_PERMISSIONS_LABEL,
                    label_width,
                ))
                .dim(),
                "YOLO mode".magenta().bold(),
            ]));
        }

        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(format!("v{}", self.version)),
            Line::from(format!(
                "model: {}{}",
                self.model,
                self.reasoning_label()
                    .map(|reasoning| format!(" {reasoning}"))
                    .unwrap_or_default()
            )),
            Line::from(format!(
                "directory: {}",
                self.format_directory(/*max_width*/ None)
            )),
        ];
        if self.yolo_mode {
            lines.push(Line::from("permissions: YOLO mode"));
        }
        lines
    }
}
