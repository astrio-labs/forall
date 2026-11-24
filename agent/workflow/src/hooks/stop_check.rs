use std::env;
use std::path::Path;

use crate::active_change::resolve_active_change;

#[derive(Debug, Clone)]
pub struct StopCheckPlan {
    pub run: bool,
    pub change: Option<String>,
}

pub fn plan_stop_check(root: &Path) -> StopCheckPlan {
    let mode = env::var("FORALL_STOP_CHECK")
        .ok()
        .map(|s| s.trim().to_lowercase());
    if mode.as_deref() == Some("off") {
        return StopCheckPlan {
            run: false,
            change: None,
        };
    }
    if mode.as_deref() == Some("always") {
        return StopCheckPlan {
            run: true,
            change: resolve_active_change(root),
        };
    }

    let change = resolve_active_change(root);
    if change.is_none() {
        return StopCheckPlan {
            run: false,
            change: None,
        };
    }
    StopCheckPlan { run: true, change }
}
