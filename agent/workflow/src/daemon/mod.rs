mod plan;
mod status;

pub use plan::{normalize_changed_paths, plan_for_changed_paths};
pub use status::{
    DaemonStatusSnapshot, DaemonTopIssue, daemon_status_path, read_daemon_status,
    write_daemon_status,
};

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::active_change::resolve_active_change;
use crate::commands::sync::{SyncOptions, run_sync};
use crate::daemon::status::{checking_snapshot, error_snapshot, snapshot_from_report};
use crate::paths::find_project_root;
use crate::verify::check_run::{CheckExecutionOptions, execute_check};
use crate::verify::report::write_report_artifacts;

pub const DEFAULT_DEBOUNCE_MS: u64 = 300;
const DEFAULT_PBT_SEED: u64 = 42;
const DEFAULT_PBT_EXAMPLES: u32 = 100;

pub struct DaemonOptions<'a> {
    pub root: Option<&'a Path>,
    pub change: Option<&'a str>,
    pub once: bool,
    pub debounce_ms: u64,
}

/// Run one daemon tick for the given changed paths (used by tests and the watch loop).
pub fn run_daemon_tick(
    root: &Path,
    change: Option<&str>,
    changed_paths: &[PathBuf],
) -> Result<DaemonStatusSnapshot, String> {
    let _ = write_daemon_status(root, &checking_snapshot(change));

    let (plan, should_sync) = if changed_paths.is_empty() {
        (crate::verify::check_run::PhasePlan::full(), false)
    } else {
        plan_for_changed_paths(root, change, changed_paths)?
    };
    if should_sync && let Some(change_name) = change {
        let _ = run_sync(SyncOptions {
            change: change_name,
            root: Some(root),
            dry_run: false,
            specs_only: false,
            annotations_only: true,
        });
    }

    let report = execute_check(CheckExecutionOptions {
        root,
        change,
        strict: false,
        pbt_seed: DEFAULT_PBT_SEED,
        pbt_examples: DEFAULT_PBT_EXAMPLES,
        plan,
        control: crate::verify::check_run::CheckExecutionControl::local(),
    })?;
    let _ = write_report_artifacts(root, &report);
    let snapshot = snapshot_from_report(change, &report);
    write_daemon_status(root, &snapshot).map_err(|e| e.to_string())?;
    Ok(snapshot)
}

fn rel_path_from_root(watch_root: &Path, event_path: &Path) -> Option<String> {
    let event = event_path
        .canonicalize()
        .unwrap_or_else(|_| event_path.to_path_buf());
    event
        .strip_prefix(watch_root)
        .ok()
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
}

fn watch_root_path(root: &Path) -> PathBuf {
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

fn log_tick_result(snapshot: &DaemonStatusSnapshot) {
    let summary = if snapshot.last_check_ok == Some(true) {
        "ok"
    } else if snapshot.last_check_ok == Some(false) {
        "FAIL"
    } else {
        "pending"
    };
    eprintln!("[forall daemon] {} {summary}", snapshot.updated_at);
    if let Some(issue) = &snapshot.top_issue {
        eprintln!(
            "  {} {}: {}",
            issue.phase,
            issue.requirement_id.as_deref().unwrap_or("-"),
            issue.message
        );
    }
}

fn run_initial_tick(root: &Path, change: Option<&str>) {
    eprintln!("forall daemon: running initial check…");
    match run_daemon_tick(root, change, &[]) {
        Ok(snapshot) => log_tick_result(&snapshot),
        Err(err) => {
            eprintln!("forall daemon: initial check failed: {err}");
            let _ = write_daemon_status(root, &error_snapshot(change, err));
        }
    }
}

fn should_watch_rel(rel: &str) -> bool {
    if rel.ends_with("daemon-status.json") || rel.contains("/reports/") {
        return false;
    }
    if rel.contains("node_modules") || rel.contains("/.git/") || rel.starts_with(".git/") {
        return false;
    }
    if rel.contains("/target/") || rel.starts_with("target/") {
        return false;
    }
    if rel.starts_with("src/") && (rel.ends_with(".ts") || rel.ends_with(".tsx")) {
        return true;
    }
    if rel.starts_with(".forall/scenarios/") {
        return rel.ends_with(".property.ts") || rel.ends_with(".property.py");
    }
    if rel.ends_with("mapping.delta.yaml") {
        return true;
    }
    rel == ".forall/verified/mapping.yaml"
}

fn is_mutating_event(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn recv_debounced(rx: &mpsc::Receiver<PathBuf>, debounce: Duration) -> Option<Vec<PathBuf>> {
    let first = match rx.recv() {
        Ok(path) => path,
        Err(_) => return None,
    };
    let mut paths = vec![first];
    let deadline = Instant::now() + debounce;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining) {
            Ok(path) => paths.push(path),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    Some(paths)
}

fn register_watches(watcher: &mut RecommendedWatcher, root: &Path) -> notify::Result<()> {
    let src = root.join("src");
    if src.is_dir() {
        watcher.watch(&src, RecursiveMode::Recursive)?;
    }
    let scenarios = root.join(".forall/scenarios");
    if scenarios.is_dir() {
        watcher.watch(&scenarios, RecursiveMode::Recursive)?;
    }
    let changes = root.join(".forall/changes");
    if changes.is_dir() {
        watcher.watch(&changes, RecursiveMode::Recursive)?;
    }
    let verified = root.join(".forall/verified");
    if verified.is_dir() {
        watcher.watch(&verified, RecursiveMode::NonRecursive)?;
    }
    Ok(())
}

pub fn run_daemon(opts: DaemonOptions<'_>) -> i32 {
    let start = opts
        .root
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let Some(root) = find_project_root(&start) else {
        eprintln!("forall daemon: project not found");
        return 1;
    };

    let change = opts
        .change
        .map(str::to_string)
        .or_else(|| resolve_active_change(&root));

    let debounce = Duration::from_millis(opts.debounce_ms.max(50));

    if opts.once {
        match run_daemon_tick(&root, change.as_deref(), &[]) {
            Ok(snapshot) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&snapshot).unwrap_or_default()
                );
                if snapshot.last_check_ok == Some(true) {
                    0
                } else {
                    1
                }
            }
            Err(err) => {
                eprintln!("forall daemon: {err}");
                let _ = write_daemon_status(&root, &error_snapshot(change.as_deref(), err));
                1
            }
        }
    } else {
        let watch_root = watch_root_path(&root);
        let (tx, rx) = mpsc::channel();
        let watch_root_for_cb = watch_root;
        let mut watcher = match RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    if !is_mutating_event(event.kind) {
                        return;
                    }
                    for path in event.paths {
                        if let Some(rel) = rel_path_from_root(&watch_root_for_cb, &path)
                            && should_watch_rel(&rel)
                        {
                            eprintln!("forall daemon: detected {rel}");
                            let _ = tx.send(path);
                        }
                    }
                }
            },
            notify::Config::default(),
        ) {
            Ok(w) => w,
            Err(err) => {
                eprintln!("forall daemon: failed to create watcher: {err}");
                let _ = write_daemon_status(
                    &root,
                    &error_snapshot(
                        change.as_deref(),
                        format!("failed to create watcher: {err}"),
                    ),
                );
                return 1;
            }
        };

        if let Err(err) = register_watches(&mut watcher, &root) {
            eprintln!("forall daemon: failed to register watches: {err}");
            let _ = write_daemon_status(
                &root,
                &error_snapshot(
                    change.as_deref(),
                    format!("failed to register watches: {err}"),
                ),
            );
            return 1;
        }

        eprintln!(
            "forall daemon: watching {} (debounce {}ms)",
            root.display(),
            debounce.as_millis()
        );
        eprintln!(
            "  status → {}",
            daemon_status_path(&root)
                .strip_prefix(&root)
                .unwrap_or(&daemon_status_path(&root))
                .display()
        );
        eprintln!(
            "forall daemon: idle — waiting for saves under src/**/*.ts(x), .forall/scenarios/*.property.*, mapping files"
        );
        if change.is_none() {
            eprintln!(
                "forall daemon: note — no active change; set .forall/.active-change or use --change"
            );
        }

        run_initial_tick(&root, change.as_deref());

        let generation = Arc::new(AtomicU64::new(0));

        loop {
            let Some(mut paths) = recv_debounced(&rx, debounce) else {
                eprintln!("forall daemon: watcher disconnected");
                return 1;
            };

            loop {
                let tick = generation.fetch_add(1, Ordering::SeqCst) + 1;
                paths.sort();
                paths.dedup();
                eprintln!(
                    "[forall daemon] check tick {tick} ({} path(s))",
                    paths.len()
                );

                let root_tick = root.clone();
                let change_tick = change.clone();
                let paths_tick = paths.clone();
                let handle = thread::spawn(move || {
                    run_daemon_tick(&root_tick, change_tick.as_deref(), &paths_tick)
                });

                let mut pending = Vec::new();
                while !handle.is_finished() {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(path) => pending.push(path),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }

                match handle.join() {
                    Ok(Ok(snapshot)) => {
                        if generation.load(Ordering::SeqCst) == tick {
                            log_tick_result(&snapshot);
                            if pending.is_empty() {
                                break;
                            }
                            paths = pending;
                        } else {
                            paths = pending;
                        }
                    }
                    Ok(Err(err)) => {
                        eprintln!("forall daemon: check failed: {err}");
                        let _ = write_daemon_status(&root, &error_snapshot(change.as_deref(), err));
                        if pending.is_empty() {
                            break;
                        }
                        paths = pending;
                    }
                    Err(_) => {
                        eprintln!("forall daemon: check thread panicked");
                        let _ = write_daemon_status(
                            &root,
                            &error_snapshot(
                                change.as_deref(),
                                "daemon check thread panicked".to_string(),
                            ),
                        );
                        if pending.is_empty() {
                            break;
                        }
                        paths = pending;
                    }
                }
            }
        }
    }
}
