//! Shared helpers for tests that mutate process-global environment variables.

use std::env;
use std::sync::Mutex;

// The process environment is global: every test that reads or writes
// FORALL_CHANGE must serialize on this one lock, or they race under the
// parallel test runner. Do not create per-module locks for this variable.
static FORALL_CHANGE_LOCK: Mutex<()> = Mutex::new(());

/// Runs `f` with `FORALL_CHANGE` set to `value` (unset for `None`), restoring
/// the previous value afterwards even if `f` panics.
pub(crate) fn with_forall_change<F: FnOnce()>(value: Option<&str>, f: F) {
    let _guard = FORALL_CHANGE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(name) => unsafe { env::set_var("FORALL_CHANGE", name) },
                None => unsafe { env::remove_var("FORALL_CHANGE") },
            }
        }
    }
    let _restore = Restore(env::var("FORALL_CHANGE").ok());

    match value {
        Some(name) => unsafe { env::set_var("FORALL_CHANGE", name) },
        None => unsafe { env::remove_var("FORALL_CHANGE") },
    }
    f();
}
