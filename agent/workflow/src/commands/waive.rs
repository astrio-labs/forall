use std::path::Path;

use crate::commands::check::{CheckOptions, run_check_report};
use crate::paths::find_project_root;
use crate::verify::report::{exit_code_for_report, print_report, write_report_artifacts};
use crate::waiver::{waiver_needed, write_waiver_record};

pub struct WaiveOptions<'a> {
    pub root: Option<&'a Path>,
    pub change: &'a str,
    pub ack: &'a str,
    pub json: bool,
}

pub fn run_waive(options: WaiveOptions<'_>) -> i32 {
    let start_dir = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&start_dir) else {
        eprintln!("forall waive: project not found");
        return 1;
    };

    let (report, _, _, _) = match run_check_report(CheckOptions {
        root: Some(&root),
        change: Some(options.change),
        strict: false,
        json: false,
        quiet: true,
        pbt_seed: 42,
        pbt_examples: 100,
    }) {
        Ok(result) => result,
        Err(code) => return code,
    };

    let _ = write_report_artifacts(&root, &report);
    if !waiver_needed(&report) {
        eprintln!("forall waive: no waivable CRITICAL issues — waiver not needed");
        if options.json {
            println!(
                "{}",
                serde_json::json!({
                    "needed": false,
                    "change": options.change,
                })
            );
        }
        return if exit_code_for_report(&report) == 0 {
            0
        } else {
            1
        };
    }

    match write_waiver_record(&root, options.change, &report, options.ack) {
        Ok(path) => {
            let rel = path
                .strip_prefix(&root)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "needed": true,
                        "change": options.change,
                        "waiverPath": rel,
                        "criticalIssueIds": crate::waiver::waivable_critical_ids(&report),
                    })
                );
            } else {
                println!(
                    "forall waive: recorded waiver for change '{}'",
                    options.change
                );
                println!("  → {rel}");
                print_report(&report);
            }
            0
        }
        Err(err) => {
            eprintln!("forall waive: {err}");
            1
        }
    }
}
