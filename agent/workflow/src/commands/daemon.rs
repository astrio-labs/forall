use std::path::Path;

use crate::daemon::{DaemonOptions, run_daemon};

pub use crate::daemon::{
    DaemonStatusSnapshot, daemon_status_path, read_daemon_status, run_daemon_tick,
};

pub struct LegacyDaemonOptions<'a> {
    pub root: Option<&'a Path>,
    pub change: Option<&'a str>,
    pub once: bool,
    pub debounce_ms: u64,
}

pub fn run_daemon_legacy(opts: LegacyDaemonOptions<'_>) -> i32 {
    run_daemon(DaemonOptions {
        root: opts.root,
        change: opts.change,
        once: opts.once,
        debounce_ms: opts.debounce_ms,
    })
}
