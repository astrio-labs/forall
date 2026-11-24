pub mod active_change;
pub mod agent_home;
pub mod assurance;
pub mod authoring;
pub mod bootstrap;
pub mod bundled;
pub mod cli;
pub mod commands;
pub mod daemon;
pub mod diagnostic;
pub mod doctor;
pub mod hooks;
pub mod mapping;
pub mod paths;
pub mod skills_install;
pub mod specs;
pub mod sync;
#[cfg(test)]
pub(crate) mod test_env;
pub mod verify;
pub mod waiver;
pub mod workflow;

pub use commands::check::run_check;
pub use commands::workflow::{
    EnsureResult, EnsureRustResult, ensure_c_verification, ensure_framac_for_agent,
    ensure_java_verification, ensure_lsc_for_agent, ensure_openjml_for_agent,
    ensure_rust_verification, ensure_verified_project, ensure_verus_for_agent,
};
pub use hooks::gate_error::{GateErrorCode, serialize_pre_tool_use_block};
pub use hooks::paths::ToolInputCarrier;
pub use hooks::pre_tool_use_gate::{
    GateError, PreToolUseGateInput, PreToolUseGateResult, evaluate_pre_tool_use_gate,
};
pub use hooks::pre_tool_use_hook::pre_tool_use_hook_output;
pub use paths::find_project_root;
pub use workflow::schema::load_workflow_schema;
pub use workflow::status::build_workflow_status;
