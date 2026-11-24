use std::path::Path;

use crate::mapping::schema::VerifyReport;
use crate::paths::find_project_root;
use crate::verify::check_run::{PhasePlan, execute_check};
use crate::verify::report::{exit_code_for_report, print_report, write_report_artifacts};

pub struct CheckOptions<'a> {
    pub root: Option<&'a Path>,
    pub change: Option<&'a str>,
    pub strict: bool,
    pub json: bool,
    pub quiet: bool,
    pub pbt_seed: u64,
    pub pbt_examples: u32,
}

pub fn run_check(options: CheckOptions<'_>) -> i32 {
    match run_check_report(options) {
        Ok((report, root, json, quiet)) => emit_check_report(&root, &report, json, quiet),
        Err(code) => code,
    }
}

pub fn run_check_report(
    options: CheckOptions<'_>,
) -> Result<(VerifyReport, std::path::PathBuf, bool, bool), i32> {
    let start_dir = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&start_dir) else {
        eprintln!("forall check: no project found (expected .forall/verified/mapping.yaml)");
        return Err(1);
    };

    let report = match execute_check(crate::verify::check_run::CheckExecutionOptions {
        root: &root,
        change: options.change,
        strict: options.strict,
        pbt_seed: options.pbt_seed,
        pbt_examples: options.pbt_examples,
        plan: PhasePlan::full(),
        control: crate::verify::check_run::CheckExecutionControl::local(),
    }) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("forall check: {err}");
            return Err(1);
        }
    };

    Ok((report, root, options.json, options.quiet))
}

fn emit_check_report(root: &Path, report: &VerifyReport, json: bool, quiet: bool) -> i32 {
    let _ = write_report_artifacts(root, report);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(report).unwrap_or_default()
        );
    } else if !quiet {
        print_report(report);
    }

    exit_code_for_report(report)
}
