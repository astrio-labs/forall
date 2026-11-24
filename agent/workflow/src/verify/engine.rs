//! Proof engine subprocess helpers — re-exports from the LemmaScript backend.
//!
//! New code should prefer `verify::backends` for multi-backend dispatch.

pub use crate::verify::backends::lemmascript::{
    lsc_check, lsc_error_detail, lsc_extract, lsc_install_hint, lsc_regen, resolve_lsc_command,
};
pub use crate::verify::backends::{BackendRunResult as LscRunResult, collect_extracts};
