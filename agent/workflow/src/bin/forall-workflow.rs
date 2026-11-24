//! Thin CLI entry for integration tests (`forall hook pre-tool-use` via stdin).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    std::process::exit(forall_workflow::cli::run_from_argv(&args));
}
