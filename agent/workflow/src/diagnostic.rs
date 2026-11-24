/// Cap extremely large subprocess output in reports (avoid multi-MB issues).
pub const MAX_DIAGNOSTIC_CHARS: usize = 32_000;

pub fn format_diagnostic_output(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= MAX_DIAGNOSTIC_CHARS {
        return trimmed.to_string();
    }
    let omitted = trimmed.len() - MAX_DIAGNOSTIC_CHARS;
    format!(
        "{}{}\n… [{omitted} characters truncated — full output exceeded {MAX_DIAGNOSTIC_CHARS}]",
        &trimmed[..MAX_DIAGNOSTIC_CHARS],
        ""
    )
}

/// Render a project-supplied string for a terminal.
///
/// `code.file` is an arbitrary string out of the project's own
/// `mapping.yaml`, and a checked-out repository is not automatically
/// trustworthy — `forall check` runs on code that arrived from somewhere.
/// Printed verbatim, a crafted path can carry an escape sequence that
/// repaints the terminal, or a newline that forges a second progress line
/// indistinguishable from one Forall wrote. A run that reports `proved` for a
/// file nobody checked is the failure this whole surface exists to prevent,
/// so the label is escaped before it reaches stderr.
///
/// Bidirectional overrides are escaped alongside the control characters: they
/// are not `char::is_control`, but they reorder how the rest of the line
/// displays, which is the same problem wearing a different hat.
pub fn terminal_safe(raw: &str) -> String {
    const BIDI_OVERRIDES: [char; 9] = [
        '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}',
        '\u{2066}', '\u{2069}',
    ];
    let mut shown = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_control() || BIDI_OVERRIDES.contains(&character) {
            shown.push_str(&format!("\\u{{{:04x}}}", character as u32));
        } else {
            shown.push(character);
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_short_output() {
        assert_eq!(
            format_diagnostic_output("line one\nline two"),
            "line one\nline two"
        );
    }

    #[test]
    fn does_not_truncate_at_500_characters() {
        let long = "x".repeat(2000);
        assert_eq!(format_diagnostic_output(&long).len(), 2000);
    }

    #[test]
    fn truncates_only_beyond_max() {
        let huge = "y".repeat(MAX_DIAGNOSTIC_CHARS + 100);
        let out = format_diagnostic_output(&huge);
        assert!(out.len() < huge.len());
        assert!(out.contains("truncated"));
    }
}
