//! Informational, warning, and policy notice history cells.

use super::*;
use crate::style::AccentStylize;

#[allow(clippy::disallowed_methods)]
pub(crate) fn new_warning_event(message: String) -> PrefixedWrappedHistoryCell {
    PrefixedWrappedHistoryCell::new(message.yellow(), "⚠ ".yellow(), "  ")
}

const TRUSTED_ACCESS_FOR_CYBER_URL: &str = "https://forall.astrio.app/dashboard";

#[derive(Debug)]
pub(crate) struct CyberPolicyNoticeCell;

pub(crate) fn new_cyber_policy_error_event() -> CyberPolicyNoticeCell {
    CyberPolicyNoticeCell
}

impl HistoryCell for CyberPolicyNoticeCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(
            vec![
                "ⓘ ".accent(),
                "This chat was flagged for possible cybersecurity risk".bold(),
            ]
            .into(),
        );

        let wrap_width = width.saturating_sub(2).max(1) as usize;
        let body = Line::from(vec![
            "  If this seems wrong, try rephrasing your request. For security-related work, visit the "
                .dim(),
            "Forall dashboard".accent().underlined(),
            " for support.".dim(),
        ]);
        let wrapped = adaptive_wrap_line(
            &body,
            RtOptions::new(wrap_width).subsequent_indent("  ".into()),
        );
        push_owned_lines(&wrapped, &mut lines);
        lines.push(
            vec![
                "  ".into(),
                TRUSTED_ACCESS_FOR_CYBER_URL.accent().underlined(),
            ]
            .into(),
        );

        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        vec![
            Line::from("This chat was flagged for possible cybersecurity risk"),
            Line::from(
                "If this seems wrong, try rephrasing your request. For security-related work, visit the Forall dashboard for support.",
            ),
            Line::from(TRUSTED_ACCESS_FOR_CYBER_URL),
        ]
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        crate::terminal_hyperlinks::annotate_web_urls(self.display_lines(width))
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }
}

#[derive(Debug)]
pub(crate) struct DeprecationNoticeCell {
    summary: String,
    details: Option<String>,
}

pub(crate) fn new_deprecation_notice(
    summary: String,
    details: Option<String>,
) -> DeprecationNoticeCell {
    DeprecationNoticeCell { summary, details }
}

impl HistoryCell for DeprecationNoticeCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(vec!["⚠ ".red().bold(), self.summary.clone().red()].into());

        let wrap_width = width.saturating_sub(4).max(1) as usize;

        if let Some(details) = &self.details {
            let detail_line = Line::from(details.clone().dim());
            let wrapped = adaptive_wrap_line(&detail_line, RtOptions::new(wrap_width));
            push_owned_lines(&wrapped, &mut lines);
        }

        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from(self.summary.clone())];
        if let Some(details) = &self.details {
            lines.extend(raw_lines_from_source(details));
        }
        lines
    }
}
pub(crate) fn new_info_event(message: String, hint: Option<String>) -> PlainHistoryCell {
    let mut line = vec!["• ".dim(), message.into()];
    if let Some(hint) = hint {
        line.push(" ".into());
        line.push(hint.dark_gray());
    }
    let lines: Vec<Line<'static>> = vec![line.into()];
    PlainHistoryCell { lines }
}

pub(crate) fn new_error_event(message: String) -> PlainHistoryCell {
    // Use a hair space (U+200A) to create a subtle, near-invisible separation
    // before the text. VS16 is intentionally omitted to keep spacing tighter
    // in terminals like Ghostty.
    let lines: Vec<Line<'static>> = vec![vec![format!("■ {message}").red()].into()];
    PlainHistoryCell { lines }
}
