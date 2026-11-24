//! Minimal Windows exec-server fixture for cross-platform tests.
//!
//! Keeping this wrapper separate avoids depending on the full Forall binary's
//! Windows cross-build, which is not yet supported by the Bazel graph. Linking
//! only the exec-server also makes the Wine test substantially faster to
//! iterate on.

use forall_exec_server::ExecServerRuntimePaths;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let current_exe = std::env::current_exe()?;
    // This fixture is always a Windows executable, so it neither invokes nor
    // needs the separate Linux sandbox binary.
    let runtime_paths =
        ExecServerRuntimePaths::new(current_exe, /*forall_linux_sandbox_exe*/ None)?;
    forall_exec_server::run_main("ws://127.0.0.1:0", runtime_paths).await
}
