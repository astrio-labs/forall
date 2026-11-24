//! Shared UI constants for layout and alignment within the TUI.

/// Width (in terminal columns) reserved for the left gutter/prefix used by
/// live cells and aligned widgets.
///
/// Semantics:
/// - Chat composer reserves this many columns for the left border + padding.
/// - User history lines account for this many columns (for example "▌ ") when wrapping.
pub(crate) const LIVE_PREFIX_COLS: u16 = 2;
/// Footer hints align with the chatbox border rather than the inner text gutter.
pub(crate) const FOOTER_INDENT_COLS: usize = 0;
