use std::path::Path;

use crate::mapping::discover::{MappingDiscoverOptions, run_mapping_discover};
use crate::paths::find_project_root;

pub struct MappingDiscoverCommandOptions<'a> {
    pub root: Option<&'a Path>,
    pub dry_run: bool,
    pub json: bool,
}

pub fn run_mapping_discover_command(options: MappingDiscoverCommandOptions<'_>) -> i32 {
    let cwd = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall mapping discover: project not found");
        return 1;
    };

    match run_mapping_discover(MappingDiscoverOptions {
        root: &root,
        dry_run: options.dry_run,
    }) {
        Ok(result) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "scannedFiles": result.scanned_files,
                        "discovered": result.discovered,
                        "skippedExisting": result.skipped_existing,
                        "skippedNoExports": result.skipped_no_exports,
                        "addedIds": result.added_ids,
                        "dryRun": options.dry_run,
                    })
                );
            } else if result.discovered == 0 {
                println!("forall mapping discover: no new exported symbols found");
                println!(
                    "  scanned {} file(s); {} already mapped; {} with no exports",
                    result.scanned_files, result.skipped_existing, result.skipped_no_exports
                );
            } else {
                let action = if options.dry_run {
                    "would discover"
                } else {
                    "discovered"
                };
                println!(
                    "forall mapping discover: {action} {} spec-tracked requirement(s) in .forall/verified/mapping.yaml",
                    result.discovered
                );
                for id in &result.added_ids {
                    println!("  + {id}");
                }
                if options.dry_run {
                    println!("  (dry run — mapping.yaml not modified)");
                } else {
                    println!("  next: edit contracts, run `forall sync`, then `forall check`");
                }
            }
            0
        }
        Err(err) => {
            eprintln!("forall mapping discover: {err}");
            1
        }
    }
}
