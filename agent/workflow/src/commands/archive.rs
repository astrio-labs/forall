use std::fs;
use std::path::Path;

use crate::commands::check::{CheckOptions, run_check_report};
use crate::mapping::load::{load_mapping, mapping_path};
use crate::paths::{ensure_dir, find_project_root};
use crate::specs::apply::{ApplyOptions, apply_specs};
use crate::verify::report::{exit_code_for_report, write_report_artifacts};
use crate::waiver::waiver_allows_archive;
use crate::workflow::paths::{archive_destination, change_dir};

fn merge_mapping_delta(root: &Path, change_name: &str) -> Result<(), String> {
    let delta_path = change_dir(root, change_name).join("mapping.delta.yaml");
    if !delta_path.is_file() {
        return Ok(());
    }

    let main_path = mapping_path(root);
    let main = if main_path.is_file() {
        load_mapping(&main_path).map_err(|e| e.to_string())?
    } else {
        crate::mapping::schema::Mapping {
            version: 1,
            requirements: Vec::new(),
        }
    };
    let delta = load_mapping(&delta_path).map_err(|e| e.to_string())?;

    // Merging must never downgrade the schema (a hardcoded `version: 1` here
    // would silently revert a migrated mapping on every archive) — and it
    // must never promote one either: pulling a v1 mapping's retained
    // `verified` claims into version 2 would fail v2 validation and block
    // the archive, and silently stripping those claims is a migration, which
    // deserves its own explicit step rather than a side effect of archiving.
    // An empty default main (no mapping file yet) adopts the delta's version.
    let main_exists = main_path.is_file();
    if main_exists && main.version != delta.version {
        return Err(format!(
            "change delta is mapping version {} but the project mapping is version {}; \
             migrate the project mapping first, or keep both at the same version",
            delta.version, main.version
        ));
    }
    let version = if main_exists {
        main.version
    } else {
        delta.version
    };

    let mut by_id = std::collections::BTreeMap::new();
    for req in main.requirements {
        by_id.insert(req.id.clone(), req);
    }
    for req in delta.requirements {
        by_id.insert(req.id.clone(), req);
    }

    let merged = crate::mapping::schema::Mapping {
        version,
        requirements: by_id.into_values().collect(),
    };
    crate::mapping::schema::validate_mapping(&merged)?;
    if let Some(parent) = main_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        &main_path,
        serde_yaml::to_string(&merged).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub struct ArchiveOptions<'a> {
    pub name: &'a str,
    pub root: Option<&'a Path>,
    pub skip_check: bool,
    pub skip_specs: bool,
    pub waiver_ack: Option<&'a str>,
}

pub fn run_archive(options: ArchiveOptions<'_>) -> i32 {
    let cwd = options
        .root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(root) = find_project_root(&cwd) else {
        eprintln!("forall archive: project not found");
        return 1;
    };

    let name = options.name;
    let src = change_dir(&root, name);
    if !src.is_dir() {
        eprintln!("forall archive: change not found: {name}");
        return 1;
    }

    if !options.skip_check {
        let (report, _, _, quiet) = match run_check_report(CheckOptions {
            root: Some(&root),
            change: Some(name),
            strict: false,
            json: false,
            quiet: false,
            pbt_seed: 42,
            pbt_examples: 100,
        }) {
            Ok(result) => result,
            Err(code) => return code,
        };
        let _ = write_report_artifacts(&root, &report);
        let exit_code = exit_code_for_report(&report);

        if exit_code != 0 {
            match waiver_allows_archive(&root, name, &report, options.waiver_ack) {
                Ok(Some(waiver_path)) => {
                    if !quiet {
                        let rel = waiver_path
                            .strip_prefix(&root)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|_| waiver_path.display().to_string());
                        eprintln!("forall archive: proceeding under explicit waiver");
                        eprintln!("  waiver: {rel}");
                        eprintln!(
                            "  tier honesty: do not claim proved, property-tested, or formally verified"
                        );
                    }
                }
                Ok(None) => {
                    if !quiet {
                        eprintln!("forall archive: proceeding under recorded waiver");
                        eprintln!(
                            "  tier honesty: do not claim proved, property-tested, or formally verified"
                        );
                    }
                }
                Err(err) => {
                    eprintln!("forall archive: verification failed; {err}");
                    return exit_code;
                }
            }
        }
    }

    // The generation gate: a change in a targeted path is not DONE until
    // its ledger evidence meets the declared minimum. This is the citation
    // requirement driving generation, not just display — the check above
    // proves what it can; this gate refuses to archive what fell short,
    // unless an unexpired waiver defers it.
    if let Err(err) = assurance_gate(&root, name) {
        eprintln!("forall archive: assurance gate failed");
        eprintln!("{err}");
        eprintln!(
            "  waive with `forall assurance waive --requirement <id> --expires-in-days <n> --reason <text>`"
        );
        return 1;
    }

    if !options.skip_specs {
        match apply_specs(
            &root,
            name,
            &ApplyOptions {
                dry_run: false,
                silent: false,
                lenient: false,
            },
        ) {
            Ok(spec_result) if spec_result.no_changes => {
                println!("forall archive: no delta specs to merge");
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("forall archive: spec merge failed: {err}");
                return 1;
            }
        }
    }

    if let Err(err) = merge_mapping_delta(&root, name) {
        eprintln!("forall archive: mapping merge failed: {err}");
        return 1;
    }

    let dest = archive_destination(&root, name);
    if dest.exists() {
        eprintln!(
            "forall archive: destination already exists: {}",
            dest.display()
        );
        return 1;
    }

    if let Some(parent) = dest.parent()
        && ensure_dir(parent).is_err()
    {
        eprintln!("forall archive: failed to create archive directory");
        return 1;
    }
    if fs::rename(&src, &dest).is_err() {
        eprintln!("forall archive: failed to move change to archive");
        return 1;
    }

    println!("forall archive: archived '{name}'");
    println!(
        "  → {}/",
        dest.strip_prefix(&root).unwrap_or(&dest).display()
    );
    println!("  specs merged (if delta specs present)");
    println!("  mapping.yaml updated from mapping.delta.yaml");
    0
}

/// Block archive while targeted paths fall below their declared minimum,
/// honoring unexpired waivers. Regressions are the ratchet's business
/// (`forall assurance check` in CI); the archive gate cares that NEW work
/// meets its targets.
fn assurance_gate(root: &Path, change: &str) -> Result<(), String> {
    use crate::assurance::{
        Finding, evaluate_with_waivers, load_baseline, load_config, load_waivers,
    };
    use crate::verify::ledger::read_ledger;

    let config = load_config(root)?;
    if config.targets.is_empty() {
        return Ok(());
    }
    let Some(ledger) = read_ledger(root, Some(change)) else {
        return Err(
            "  no verify ledger for this change — run `forall check --change` first".to_string(),
        );
    };
    let mut baseline = load_baseline(root)?;
    let waivers = load_waivers(root)?;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let report = evaluate_with_waivers(&config, &mut baseline, &ledger, &waivers, &today);

    let below: Vec<String> = report
        .findings
        .iter()
        .filter_map(|finding| match finding {
            Finding::BelowTarget {
                requirement_id,
                file,
                target,
                earned,
            } => Some(format!(
                "  {requirement_id} ({file}): earned {} — target {}",
                earned.as_str(),
                target.as_str()
            )),
            Finding::Regression { .. } => None,
        })
        .collect();
    if below.is_empty() {
        Ok(())
    } else {
        Err(below.join("\n"))
    }
}

#[cfg(test)]
mod mapping_merge_tests {
    use super::*;
    use crate::workflow::paths::change_dir;

    fn write_delta(root: &Path, change: &str, content: &str) {
        let dir = change_dir(root, change);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mapping.delta.yaml"), content).unwrap();
    }

    fn write_main(root: &Path, content: &str) {
        let path = mapping_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    const V1_MAIN: &str = "version: 1\nrequirements:\n  - id: legacy\n    capability: core\n    requirement: kept\n    verified: true\n";
    const V1_DELTA: &str =
        "version: 1\nrequirements:\n  - id: fresh\n    capability: core\n    requirement: added\n";
    const V2_DELTA: &str =
        "version: 2\nrequirements:\n  - id: fresh\n    capability: core\n    requirement: added\n";

    #[test]
    fn same_version_merge_preserves_the_version() {
        let dir = tempfile::tempdir().unwrap();
        write_main(dir.path(), V1_MAIN);
        write_delta(dir.path(), "change", V1_DELTA);

        merge_mapping_delta(dir.path(), "change").unwrap();
        let merged = load_mapping(&mapping_path(dir.path())).unwrap();
        assert_eq!(merged.version, 1);
        assert_eq!(merged.requirements.len(), 2);
    }

    #[test]
    fn mixed_version_merge_is_refused_not_silently_migrated() {
        let dir = tempfile::tempdir().unwrap();
        write_main(dir.path(), V1_MAIN);
        write_delta(dir.path(), "change", V2_DELTA);

        let err = merge_mapping_delta(dir.path(), "change").unwrap_err();
        assert!(err.contains("migrate"), "got: {err}");
        // The refusal must leave the main mapping untouched.
        let main = load_mapping(&mapping_path(dir.path())).unwrap();
        assert_eq!(main.version, 1);
        assert_eq!(main.requirements.len(), 1);
    }

    #[test]
    fn missing_main_adopts_the_delta_version() {
        let dir = tempfile::tempdir().unwrap();
        write_delta(dir.path(), "change", V2_DELTA);

        merge_mapping_delta(dir.path(), "change").unwrap();
        let merged = load_mapping(&mapping_path(dir.path())).unwrap();
        assert_eq!(merged.version, 2);
        assert_eq!(merged.requirements.len(), 1);
    }
}
