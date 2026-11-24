//! Prominent, compact verification summaries shown after Forall checks.

use super::*;

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationUiStatus {
    Fresh,
    Stale,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerificationUiSummary {
    pub(crate) state: VerificationUiStatus,
    /// Refines a stale state into WHAT drifted; open string on the wire.
    #[serde(default)]
    pub(crate) stale_grade: Option<String>,
    #[serde(default)]
    pub(crate) proved: usize,
    #[serde(default)]
    pub(crate) property_tested: usize,
    #[serde(default)]
    pub(crate) spec_tracked: usize,
    #[serde(default)]
    pub(crate) total_requirements: usize,
    #[serde(default)]
    pub(crate) critical_issues: usize,
    #[serde(default)]
    pub(crate) change: Option<String>,
    #[serde(default)]
    pub(crate) report_path: Option<String>,
}

impl VerificationUiSummary {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        serde_json::from_str(raw).ok()
    }

    fn pct(count: usize, total: usize) -> Option<u32> {
        ((count * 100) + (total / 2))
            .checked_div(total)
            .map(|pct| pct as u32)
    }

    fn format_count_with_pct(&self, label: &str, count: usize) -> String {
        match Self::pct(count, self.total_requirements) {
            Some(pct) => format!("{count} {label} ({pct}%)"),
            None => format!("{count} {label}"),
        }
    }

    fn detailed_counts(&self) -> String {
        [
            self.format_count_with_pct("proved", self.proved),
            self.format_count_with_pct("property-tested", self.property_tested),
            self.format_count_with_pct("spec-tracked", self.spec_tracked),
        ]
        .join(" · ")
    }

    fn requirements_total_line(&self) -> Option<String> {
        if self.total_requirements == 0 {
            return None;
        }
        Some(format!("{} requirements in scope", self.total_requirements))
    }

    /// Whether to insert a transcript card for this summary.
    ///
    /// Unchanged results are suppressed so casual follow-up turns do not repeat the
    /// same verification block. Failures and stale states resurface when they change.
    pub(crate) fn should_show_card(&self, last_shown: Option<&Self>) -> bool {
        last_shown != Some(self)
    }

    fn compact_success_line(&self) -> String {
        self.detailed_counts()
    }

    /// Success copy that does not over-claim formal verification.
    ///
    /// "Verification passed" is reserved for turns with proved or property-tested
    /// requirements. Spec-tracked-only workflow checks use plainer wording so a
    /// casual chat turn does not look like code was formally verified.
    fn fresh_success_heading(&self) -> &'static str {
        if self.proved > 0 || self.property_tested > 0 {
            "Verification passed"
        } else if self.spec_tracked > 0 {
            "Specs tracked"
        } else {
            "Check passed"
        }
    }

    fn fresh_success_footnote(&self) -> &'static str {
        if self.proved > 0 || self.property_tested > 0 {
            "All checks passed"
        } else if self.spec_tracked > 0 {
            "Workflow/mapping only — not formally proved"
        } else {
            "No requirements in scope yet"
        }
    }
}

#[derive(Debug)]
pub(crate) struct VerificationSummaryCell {
    summary: VerificationUiSummary,
}

impl VerificationSummaryCell {
    pub(crate) fn new(summary: VerificationUiSummary) -> Self {
        Self { summary }
    }

    fn lines(&self) -> Vec<Line<'static>> {
        match self.summary.state {
            VerificationUiStatus::Fresh => vec![Line::from(vec![
                "✓ ".green().bold(),
                self.summary.fresh_success_heading().green().bold(),
                " · ".dim(),
                self.summary.compact_success_line().into(),
            ])],
            VerificationUiStatus::Failed | VerificationUiStatus::Stale => self.detailed_lines(),
        }
    }

    fn detailed_lines(&self) -> Vec<Line<'static>> {
        let (symbol, heading) = match self.summary.state {
            VerificationUiStatus::Fresh => ("✓", self.summary.fresh_success_heading()),
            VerificationUiStatus::Failed => ("✗", "Verification blocked"),
            // The grade says WHAT drifted, which decides what the user does
            // next: re-prove code, re-check the claim, or just replay.
            VerificationUiStatus::Stale => (
                "!",
                match self.summary.stale_grade.as_deref() {
                    Some("code_drift") => "Verification stale — code changed since the proof",
                    Some("spec_drift") => "Verification stale — the claim changed, not the code",
                    Some("toolchain_drift") => "Verification stale — prover updated, replay owed",
                    Some("unknown") => "Verification stale — citations could not be re-anchored",
                    _ => "Verification stale",
                },
            ),
        };
        let heading = match self.summary.state {
            VerificationUiStatus::Fresh => Line::from(vec![
                format!("{symbol} ").green().bold(),
                heading.green().bold(),
            ]),
            VerificationUiStatus::Failed => Line::from(vec![
                format!("{symbol} ").red().bold(),
                heading.red().bold(),
            ]),
            VerificationUiStatus::Stale => Line::from(vec![
                format!("{symbol} ").cyan().bold(),
                heading.cyan().bold(),
            ]),
        };

        let mut lines = vec![
            heading,
            Line::from(vec!["  ".into(), self.summary.detailed_counts().into()]),
        ];

        if let Some(total_line) = self.summary.requirements_total_line() {
            lines.push(Line::from(vec!["  ".into(), total_line.dim()]));
        }

        match self.summary.state {
            VerificationUiStatus::Fresh => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    self.summary.fresh_success_footnote().dim(),
                ]));
            }
            VerificationUiStatus::Failed => {
                let noun = if self.summary.critical_issues == 1 {
                    "critical issue"
                } else {
                    "critical issues"
                };
                let mut spans = vec![
                    "  ".into(),
                    format!("{} {noun}", self.summary.critical_issues)
                        .red()
                        .bold(),
                ];
                if let Some(path) = self.summary.report_path.as_deref() {
                    spans.push(" · report: ".dim());
                    spans.push(path.to_string().underlined());
                }
                lines.push(Line::from(spans));
            }
            VerificationUiStatus::Stale => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    "Run a check to refresh this result".dim(),
                ]));
            }
        }
        lines
    }
}

impl HistoryCell for VerificationSummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        adaptive_wrap_lines(self.lines(), RtOptions::new(usize::from(width.max(1))))
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(self.lines())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: Vec<Line<'static>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn renders_mixed_success_summary() {
        let cell = VerificationSummaryCell::new(VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 4,
            property_tested: 2,
            spec_tracked: 1,
            total_requirements: 7,
            critical_issues: 0,
            change: Some("demo".into()),
            report_path: None,
        });
        assert_eq!(
            texts(cell.raw_lines()),
            vec![
                "✓ Verification passed · 4 proved (57%) · 2 property-tested (29%) · 1 spec-tracked (14%)"
            ],
        );
    }

    #[test]
    fn renders_spec_tracked_only_without_claiming_verification() {
        let cell = VerificationSummaryCell::new(VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 0,
            property_tested: 0,
            spec_tracked: 2,
            total_requirements: 2,
            critical_issues: 0,
            change: Some("demo".into()),
            report_path: None,
        });
        assert_eq!(
            texts(cell.raw_lines()),
            vec![
                "✓ Specs tracked · 0 proved (0%) · 0 property-tested (0%) · 2 spec-tracked (100%)"
            ],
        );
    }

    #[test]
    fn suppresses_unchanged_card() {
        let summary = VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 4,
            property_tested: 0,
            spec_tracked: 0,
            total_requirements: 4,
            critical_issues: 0,
            change: None,
            report_path: None,
        };
        assert!(summary.should_show_card(None));
        assert!(!summary.should_show_card(Some(&summary)));
    }

    #[test]
    fn reshows_when_counts_change() {
        let first = VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 4,
            property_tested: 0,
            spec_tracked: 0,
            total_requirements: 4,
            critical_issues: 0,
            change: None,
            report_path: None,
        };
        let updated = VerificationUiSummary {
            proved: 5,
            total_requirements: 5,
            ..first.clone()
        };
        assert!(!first.should_show_card(Some(&first)));
        assert!(updated.should_show_card(Some(&first)));
    }

    #[test]
    fn pct_rounds_to_nearest_integer() {
        let summary = VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 1,
            property_tested: 0,
            spec_tracked: 2,
            total_requirements: 3,
            critical_issues: 0,
            change: None,
            report_path: None,
        };
        assert_eq!(
            summary.detailed_counts(),
            "1 proved (33%) · 0 property-tested (0%) · 2 spec-tracked (67%)"
        );
    }

    #[test]
    fn omits_percentages_without_requirement_total() {
        let summary = VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Fresh,
            proved: 2,
            property_tested: 1,
            spec_tracked: 0,
            total_requirements: 0,
            critical_issues: 0,
            change: None,
            report_path: None,
        };
        assert_eq!(
            summary.detailed_counts(),
            "2 proved · 1 property-tested · 0 spec-tracked"
        );
    }

    #[test]
    fn renders_failed_summary_with_report() {
        let cell = VerificationSummaryCell::new(VerificationUiSummary {
            stale_grade: None,
            state: VerificationUiStatus::Failed,
            proved: 3,
            property_tested: 1,
            spec_tracked: 2,
            total_requirements: 6,
            critical_issues: 1,
            change: Some("demo".into()),
            report_path: Some(".forall/verify/cache/reports/demo-verify.json".into()),
        });
        assert_eq!(
            texts(cell.raw_lines()),
            vec![
                "✗ Verification blocked",
                "  3 proved (50%) · 1 property-tested (17%) · 2 spec-tracked (33%)",
                "  6 requirements in scope",
                "  1 critical issue · report: .forall/verify/cache/reports/demo-verify.json",
            ]
        );
    }
}
