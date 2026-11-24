use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

use crate::branding::ONBOARDING_SYMBOL_ASCII_ART;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepStateProvider;

use super::onboarding_screen::StepState;

pub(crate) struct WelcomeWidget {
    pub is_logged_in: bool,
}

impl KeyboardHandler for WelcomeWidget {
    fn handle_key_event(&mut self, _key_event: crossterm::event::KeyEvent) {}
}

impl WelcomeWidget {
    pub(crate) fn new(is_logged_in: bool) -> Self {
        Self { is_logged_in }
    }
}

impl WidgetRef for &WelcomeWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);

        let mut lines: Vec<Line> = ONBOARDING_SYMBOL_ASCII_ART
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect();
        lines.push("".into());
        lines.push(Line::from(vec![
            "  ".into(),
            "Welcome to ".into(),
            "Forall".bold(),
            ", verified coding agent".into(),
        ]));

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(area, buf);
    }
}

impl StepStateProvider for WelcomeWidget {
    fn get_step_state(&self) -> StepState {
        match self.is_logged_in {
            true => StepState::Hidden,
            false => StepState::Complete,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn row_containing(buf: &Buffer, needle: &str) -> Option<u16> {
        (0..buf.area.height).find(|&y| {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            row.contains(needle)
        })
    }

    #[test]
    fn welcome_renders_forall_ascii_symbol_and_tagline() {
        let widget = WelcomeWidget::new(/*is_logged_in*/ false);
        let art_lines = ONBOARDING_SYMBOL_ASCII_ART.lines().count() as u16;
        let area = Rect::new(0, 0, 60, art_lines + 4);
        let mut buf = Buffer::empty(area);
        (&widget).render(area, &mut buf);

        assert!(
            row_containing(&buf, "∀").is_some(),
            "expected forall symbol in onboarding art"
        );
        assert_eq!(row_containing(&buf, "Welcome"), Some(art_lines + 1));
    }
}
