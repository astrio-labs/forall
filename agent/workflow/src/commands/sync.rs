use std::path::Path;

use crate::mapping::load::load_mapping;
use crate::paths::find_project_root;
use crate::specs::apply::{ApplyOptions, apply_specs};
use crate::sync::annotations::{AnnotationSyncResult, sync_annotations};
use crate::verify::adapters::{PROOF_LEMMASCRIPT_DAFNY, load_verified_config};
use crate::verify::dfy_merge::{format_lsc_regen_failure, lsc_regen_clean};
use crate::workflow::paths::change_dir;

pub struct SyncOptions<'a> {
    pub change: &'a str,
    pub root: Option<&'a Path>,
    pub dry_run: bool,
    pub specs_only: bool,
    pub annotations_only: bool,
}

pub fn run_sync(options: SyncOptions<'_>) -> i32 {
    let cwd = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall sync: project not found");
        return 1;
    };

    let change_root = change_dir(&root, options.change);
    if !change_root.is_dir() {
        eprintln!("forall sync: change not found: {}", options.change);
        return 1;
    }

    let do_specs = !options.annotations_only;
    let do_annotations = !options.specs_only;

    if do_specs {
        match apply_specs(
            &root,
            options.change,
            &ApplyOptions {
                dry_run: options.dry_run,
                silent: false,
                lenient: true,
            },
        ) {
            Ok(result) if result.no_changes => {
                println!("forall sync: no delta specs to apply");
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("forall sync: {err}");
                return 1;
            }
        }
    }

    if do_annotations {
        let delta_path = change_root.join("mapping.delta.yaml");
        if !delta_path.is_file() {
            println!("forall sync: no mapping.delta.yaml (skipped annotations)");
        } else if options.dry_run {
            println!("forall sync: would stub annotations from mapping.delta.yaml");
        } else {
            let delta = match load_mapping(&delta_path) {
                Ok(d) => d,
                Err(err) => {
                    eprintln!("forall sync: {err}");
                    return 1;
                }
            };
            let AnnotationSyncResult {
                files_updated,
                stubs_added,
                skipped,
            } = sync_annotations(&root, &delta.requirements);
            if stubs_added > 0 {
                println!(
                    "forall sync: added {stubs_added} annotation stub(s) in {} file(s)",
                    files_updated.len()
                );
                let config = load_verified_config(&root);
                for file in files_updated {
                    let adapter = config
                        .adapter_for_file(&file)
                        .map(|(_, adapter)| adapter.proof.as_str());
                    if adapter != Some(PROOF_LEMMASCRIPT_DAFNY) {
                        continue;
                    }
                    let regen = lsc_regen_clean(&root, &file);
                    if !regen.ok {
                        eprintln!("forall sync: {}", format_lsc_regen_failure(&file, &regen));
                        return 1;
                    }
                }
            } else {
                println!("forall sync: annotations already present (no stubs added)");
            }
            for note in skipped {
                eprintln!("forall sync: skipped {note}");
            }
        }
    }

    0
}
