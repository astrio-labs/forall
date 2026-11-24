//! Status output formatting and display adapters for the TUI.
//!
//! This module turns runtime state into stable display structures used by `/status`
//! output and footer/status-line helpers, while keeping rendering concerns out of transport-facing
//! code.
mod account;
mod card;
mod format;
mod helpers;
pub(crate) mod remote_connection;

pub(crate) use account::StatusAccountDisplay;
#[cfg(test)]
pub(crate) use card::new_status_output;
pub(crate) use card::new_status_output_handle;
pub(crate) use helpers::compose_agents_summary;
pub(crate) use helpers::format_directory_display;
pub(crate) use helpers::format_tokens_compact;

#[cfg(test)]
mod tests;
