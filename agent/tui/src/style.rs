use crate::color::blend;
use crate::color::is_light;
use crate::terminal_palette::StdoutColorLevel;
use crate::terminal_palette::best_color;
use crate::terminal_palette::default_bg;
use crate::terminal_palette::default_fg;
use crate::terminal_palette::rgb_color;
use crate::terminal_palette::stdout_color_level;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Span;

/// Forall brand primary accent (#4BA6DB).
pub(crate) const PRIMARY_ACCENT_RGB: (u8, u8, u8) = (0x4B, 0xA6, 0xDB);
// Decorative table rules should remain visible without competing with cell content.
const TABLE_SEPARATOR_FG_ALPHA: f32 = 0.20;
// Chat composer border: visible but quieter than accent controls.
const COMPOSER_BORDER_FG_ALPHA: f32 = 0.35;

pub fn user_message_style() -> Style {
    user_message_style_for(default_bg())
}

pub fn proposed_plan_style() -> Style {
    proposed_plan_style_for(default_bg())
}

/// Returns a low-contrast rule style for separators within markdown tables.
pub(crate) fn table_separator_style() -> Style {
    table_separator_style_for(default_fg(), default_bg(), stdout_color_level())
}

/// Returns the border style for the chat composer prompt box (no background fill).
pub(crate) fn composer_border_style() -> Style {
    composer_border_style_for(default_fg(), default_bg(), stdout_color_level())
}

/// Returns the shared primary accent foreground color.
pub(crate) fn primary_accent_fg() -> Color {
    primary_accent_fg_for(stdout_color_level())
}

fn primary_accent_fg_for(_color_level: StdoutColorLevel) -> Color {
    // Always emit truecolor RGB for the brand accent. Approximating via
    // ANSI-256/16 often lands on a bright cyan that no longer matches #4BA6DB.
    rgb_color(PRIMARY_ACCENT_RGB)
}

/// Primary accent foreground without bold (drop-in for legacy `.cyan()` styling).
pub(crate) fn primary_accent_fg_style() -> Style {
    Style::default().fg(primary_accent_fg())
}

/// Returns the shared primary accent style for active or selected TUI controls.
pub(crate) fn primary_accent_style() -> Style {
    primary_accent_style_for(stdout_color_level())
}

fn primary_accent_style_for(color_level: StdoutColorLevel) -> Style {
    Style::default()
        .fg(primary_accent_fg_for(color_level))
        .bold()
}

/// Stylize helper replacing ratatui `.cyan()` with the Forall brand accent.
pub(crate) trait AccentStylize {
    fn accent(self) -> Span<'static>;
}

impl AccentStylize for &str {
    fn accent(self) -> Span<'static> {
        Span::styled(self.to_string(), primary_accent_fg_style())
    }
}

impl AccentStylize for String {
    fn accent(self) -> Span<'static> {
        Span::styled(self, primary_accent_fg_style())
    }
}

impl AccentStylize for std::borrow::Cow<'_, str> {
    fn accent(self) -> Span<'static> {
        Span::styled(self.into_owned(), primary_accent_fg_style())
    }
}

/// Returns the style for the chat composer prompt bar and user-message accent.
pub(crate) fn composer_prompt_style() -> Style {
    primary_accent_style()
}

/// Left accent bar shown before user input and submitted user messages.
pub(crate) fn composer_prompt_bar_span() -> Span<'static> {
    Span::styled("▌", composer_prompt_style())
}

/// Returns the shared accent style for active or selected TUI controls.
pub(crate) fn accent_style() -> Style {
    primary_accent_style()
}

/// Returns the style for a user-authored message using the provided terminal background.
pub fn user_message_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(user_message_bg(bg)),
        None => Style::default(),
    }
}

pub fn proposed_plan_style_for(terminal_bg: Option<(u8, u8, u8)>) -> Style {
    match terminal_bg {
        Some(bg) => Style::default().bg(proposed_plan_bg(bg)),
        None => Style::default(),
    }
}

fn muted_fg_style_for(
    terminal_fg: Option<(u8, u8, u8)>,
    terminal_bg: Option<(u8, u8, u8)>,
    color_level: StdoutColorLevel,
    alpha: f32,
) -> Style {
    let (Some(fg), Some(bg)) = (terminal_fg, terminal_bg) else {
        return Style::default().dim();
    };
    let muted_rgb = blend(fg, bg, alpha);
    match color_level {
        StdoutColorLevel::TrueColor => Style::default().fg(rgb_color(muted_rgb)),
        StdoutColorLevel::Ansi256 => Style::default().fg(best_color(muted_rgb)),
        StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => Style::default().dim(),
    }
}

fn table_separator_style_for(
    terminal_fg: Option<(u8, u8, u8)>,
    terminal_bg: Option<(u8, u8, u8)>,
    color_level: StdoutColorLevel,
) -> Style {
    muted_fg_style_for(
        terminal_fg,
        terminal_bg,
        color_level,
        TABLE_SEPARATOR_FG_ALPHA,
    )
}

fn composer_border_style_for(
    terminal_fg: Option<(u8, u8, u8)>,
    terminal_bg: Option<(u8, u8, u8)>,
    color_level: StdoutColorLevel,
) -> Style {
    muted_fg_style_for(
        terminal_fg,
        terminal_bg,
        color_level,
        COMPOSER_BORDER_FG_ALPHA,
    )
}

#[allow(clippy::disallowed_methods)]
pub fn user_message_bg(terminal_bg: (u8, u8, u8)) -> Color {
    let (top, alpha) = if is_light(terminal_bg) {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), 0.12)
    };
    best_color(blend(top, terminal_bg, alpha))
}

#[allow(clippy::disallowed_methods)]
pub fn proposed_plan_bg(terminal_bg: (u8, u8, u8)) -> Color {
    user_message_bg(terminal_bg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn accent_style_uses_brand_primary_truecolor() {
        let expected = Style::default().fg(rgb_color(PRIMARY_ACCENT_RGB)).bold();

        assert_eq!(accent_style(), expected);
        assert_eq!(primary_accent_fg(), rgb_color(PRIMARY_ACCENT_RGB));
    }

    #[test]
    fn table_separator_blends_toward_dark_background() {
        let style = table_separator_style_for(
            Some((255, 255, 255)),
            Some((0, 0, 0)),
            StdoutColorLevel::TrueColor,
        );

        assert_eq!(style.fg, Some(rgb_color((51, 51, 51))));
    }

    #[test]
    fn table_separator_blends_toward_light_background() {
        let style = table_separator_style_for(
            Some((0, 0, 0)),
            Some((255, 255, 255)),
            StdoutColorLevel::TrueColor,
        );

        assert_eq!(style.fg, Some(rgb_color((204, 204, 204))));
    }

    #[test]
    fn composer_border_is_more_visible_than_table_separator_on_dark_background() {
        let separator = table_separator_style_for(
            Some((255, 255, 255)),
            Some((0, 0, 0)),
            StdoutColorLevel::TrueColor,
        );
        let border = composer_border_style_for(
            Some((255, 255, 255)),
            Some((0, 0, 0)),
            StdoutColorLevel::TrueColor,
        );

        assert_eq!(separator.fg, Some(rgb_color((51, 51, 51))));
        assert_eq!(border.fg, Some(rgb_color((89, 89, 89))));
    }

    #[test]
    fn table_separator_dims_when_palette_aware_color_is_unavailable() {
        let expected = Style::default().dim();

        assert_eq!(
            table_separator_style_for(
                Some((255, 255, 255)),
                Some((0, 0, 0)),
                StdoutColorLevel::Ansi16,
            ),
            expected
        );
        assert_eq!(
            table_separator_style_for(
                /*terminal_fg*/ None,
                Some((0, 0, 0)),
                StdoutColorLevel::TrueColor,
            ),
            expected
        );
    }
}
