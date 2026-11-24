use std::path::Path;

use crate::mapping::import::{MappingImportOptions, run_mapping_import};
use crate::paths::find_project_root;

pub struct MappingImportCommandOptions<'a> {
    pub root: Option<&'a Path>,
    pub dry_run: bool,
    pub json: bool,
}

pub fn run_mapping_import_command(options: MappingImportCommandOptions<'_>) -> i32 {
    let cwd = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall mapping import: project not found");
        return 1;
    };

    match run_mapping_import(MappingImportOptions {
        root: &root,
        dry_run: options.dry_run,
    }) {
        Ok(result) => {
            if options.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "scannedFiles": result.scanned_files,
                        "imported": result.imported,
                        "skippedExisting": result.skipped_existing,
                        "skippedNoAnnotations": result.skipped_no_annotations,
                        "extractFailures": result.extract_failures,
                        "addedIds": result.added_ids,
                        "dryRun": options.dry_run,
                    })
                );
            } else if result.imported == 0 {
                println!("forall mapping import: no new annotated symbols found");
                println!(
                    "  scanned {} TypeScript file(s); {} extract failure(s); {} already mapped",
                    result.scanned_files, result.extract_failures, result.skipped_existing
                );
            } else {
                let action = if options.dry_run {
                    "would import"
                } else {
                    "imported"
                };
                println!(
                    "forall mapping import: {action} {} requirement(s) into .forall/verified/mapping.yaml",
                    result.imported
                );
                for id in &result.added_ids {
                    println!("  + {id}");
                }
                if options.dry_run {
                    println!("  (dry run — mapping.yaml not modified)");
                }
            }
            0
        }
        Err(err) => {
            eprintln!("forall mapping import: {err}");
            1
        }
    }
}
