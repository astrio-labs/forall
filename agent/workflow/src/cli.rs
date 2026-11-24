use std::io::{self, Read};
use std::path::Path;

use clap::{Parser, Subcommand};

use crate::active_change::resolve_active_change;
use crate::commands::archive::{ArchiveOptions, run_archive};
use crate::commands::check::{CheckOptions, run_check};
use crate::commands::hooks_install::{run_hooks_install, run_hooks_print};
use crate::commands::mapping_discover::run_mapping_discover_command;
use crate::commands::mapping_import::run_mapping_import_command;
use crate::commands::mapping_validate::run_mapping_validate;
use crate::commands::status::{run_instructions, run_status, run_workflow_status};
use crate::commands::sync::{SyncOptions, run_sync};
use crate::commands::waive::{WaiveOptions, run_waive};
use crate::commands::workflow::{
    ensure_c_for_agent, ensure_java_for_agent, ensure_lsc_for_agent, ensure_rust_verification,
    ensure_verified_project, ensure_verus_for_agent, run_ensure, run_ensure_c, run_ensure_framac,
    run_ensure_dafny, run_ensure_java, run_ensure_lsc, run_ensure_openjml, run_ensure_rust, run_ensure_verus,
    run_init, run_list, run_propose,
};
use crate::daemon::run_daemon;
use crate::hooks::browser_claims::{BrowserClaimInput, validate_browser_verification_claims};
use crate::hooks::browser_session::record_browser_evidence;
use crate::hooks::gate_error::write_pre_tool_use_block;
use crate::hooks::hook_input::project_root;
use crate::hooks::paths::tool_input_from_json;
use crate::hooks::pre_tool_use_hook::pre_tool_use_hook_output;
use crate::hooks::stop_check::plan_stop_check;
use crate::hooks::stop_success::build_stop_check_success_message;
use crate::hooks::tier_claims::{TierClaimInput, validate_tier_honesty_claims};
use crate::mapping::schema::{IssueSeverity, VerificationSummary, VerifyReport};
use crate::paths::find_project_root;
use crate::paths::verify_reports_dir;
use crate::verify::check_inputs::check_inputs_unchanged;
use crate::verify::report::{format_verification_tier, read_verify_report};
use crate::workflow::paths::list_active_changes;
use crate::workflow::schema::load_workflow_schema;
use crate::workflow::state::compute_change_status;
use crate::workflow::status::build_workflow_status;

#[derive(Parser)]
#[command(name = "forall", about = "Forall — coding agent CLI", version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Bootstrap {
        #[arg(long)]
        root: Option<String>,
    },
    Init {
        /// Scaffold a Verus Rust crate (requires `cargo verus` on PATH).
        #[arg(long)]
        rust: bool,
        /// Install optional `.forall/AGENTS.md` reference doc (rules live in the agent).
        #[arg(long)]
        agents_md: bool,
    },
    Ensure {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    EnsureRust {
        #[arg(long)]
        root: Option<String>,
        /// Scaffold a Verus crate when `.rs` files exist but no root Cargo.toml.
        #[arg(long)]
        scaffold: bool,
        #[arg(long)]
        json: bool,
    },
    EnsureJava {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    EnsureC {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    EnsureVerus {
        #[arg(long)]
        root: Option<String>,
        /// Check only; do not download/install.
        #[arg(long)]
        check_only: bool,
        #[arg(long)]
        json: bool,
    },
    EnsureDafny {
        #[arg(long)]
        root: Option<String>,
        /// Check only; do not download/install.
        #[arg(long)]
        check_only: bool,
        #[arg(long)]
        json: bool,
    },
    EnsureLsc {
        #[arg(long)]
        root: Option<String>,
        /// Check only; do not download/install.
        #[arg(long)]
        check_only: bool,
        #[arg(long)]
        json: bool,
    },
    EnsureOpenjml {
        #[arg(long)]
        root: Option<String>,
        /// OpenJML install directory (`OpenJMLsrc/`) or `openjml` script to link.
        #[arg(long)]
        path: Option<String>,
        /// Check only; do not link from `FORALL_OPENJML_HOME` candidates or download a release.
        #[arg(long)]
        check_only: bool,
        #[arg(long)]
        json: bool,
    },
    EnsureFramac {
        #[arg(long)]
        root: Option<String>,
        /// Frama-C binary or install directory (e.g. an opam switch) to link.
        #[arg(long)]
        path: Option<String>,
        /// Check only; do not link from `FORALL_FRAMAC_HOME` candidates.
        #[arg(long)]
        check_only: bool,
        #[arg(long)]
        json: bool,
    },
    GetWorkflowStatus {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
    },
    Propose {
        name: String,
        /// Collapsed workflow: proposal + specs + tasks only (skips design and verification).
        #[arg(long)]
        micro: bool,
    },
    List {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Instructions {
        artifact: String,
        #[arg(long)]
        change: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        plain: bool,
    },
    Archive {
        name: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        skip_check: bool,
        #[arg(long)]
        skip_specs: bool,
        /// User confirmation text when archiving despite waivable CRITICAL issues.
        #[arg(long)]
        ack: Option<String>,
    },
    Waive {
        #[arg(long)]
        change: String,
        /// Quote or summary of explicit user consent.
        #[arg(long)]
        ack: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Sync {
        #[arg(long)]
        change: String,
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        specs_only: bool,
        #[arg(long)]
        annotations_only: bool,
    },
    Check {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
        #[arg(long)]
        strict: bool,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value = "42")]
        pbt_seed: u64,
        #[arg(long, default_value = "100")]
        pbt_examples: u32,
    },
    /// Export audit artifacts derived from the verify ledger.
    Export {
        #[command(subcommand)]
        command: ExportCommands,
    },
    /// Assurance targets and the evidence ratchet.
    Assurance {
        #[command(subcommand)]
        command: AssuranceCommands,
    },
    Mapping {
        #[command(subcommand)]
        command: MappingCommands,
    },
    Hook {
        #[command(subcommand)]
        command: HookCommands,
    },
    Daemon {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
        #[arg(long)]
        once: bool,
        #[arg(long, default_value_t = crate::daemon::DEFAULT_DEBOUNCE_MS)]
        debounce_ms: u64,
    },
    Hooks {
        #[command(subcommand)]
        command: HooksCommands,
    },
}

#[derive(Subcommand)]
enum ExportCommands {
    /// Requirements-traceability matrix (CSV) from mapping + ledger.
    Rtm {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
        #[arg(long)]
        out: Option<String>,
    },
}

#[derive(Subcommand)]
enum AssuranceCommands {
    /// Gate on the committed baseline and per-path targets.
    Check {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        change: Option<String>,
    },
    /// Record a time-boxed exception for one requirement.
    Waive {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        requirement: String,
        /// Days until the waiver expires (mandatory: waivers are deferrals,
        /// not policy).
        #[arg(long)]
        expires_in_days: u32,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum MappingCommands {
    Validate {
        #[arg(long)]
        root: Option<String>,
    },
    /// Migrate mapping.yaml to version 2: strip agent-writable evidence
    /// claims; evidence is earned via `forall check` into the ledger.
    Migrate {
        #[arg(long)]
        root: Option<String>,
    },
    Import {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
    /// Scan exported symbols and add spec-tracked requirements (no //@ needed).
    Discover {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HookCommands {
    SessionStart,
    PreToolUse,
    PostEdit,
    PostToolUseBrowser,
    Stop,
}

#[derive(Subcommand)]
enum HooksCommands {
    Install {
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        global: bool,
    },
    Print,
}

fn path_opt(s: &Option<String>) -> Option<&Path> {
    s.as_deref().map(Path::new)
}

pub fn run_from_argv(argv: &[String]) -> i32 {
    let tail: Vec<&str> = if argv.len() > 1 {
        argv.iter().skip(1).map(String::as_str).collect()
    } else {
        argv.iter().map(String::as_str).collect()
    };
    let cli = match Cli::try_parse_from(std::iter::once("forall").chain(tail)) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("{err}");
            return 2;
        }
    };

    match cli.command {
        Commands::Bootstrap { root } => {
            let cwd = path_opt(&root)
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            if crate::bootstrap::bootstrap_forall_integration(&cwd, false, false).is_ok() {
                0
            } else {
                1
            }
        }
        Commands::Init { rust, agents_md } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            run_init(&cwd, rust, false, agents_md)
        }
        Commands::Ensure { root, json } => run_ensure(path_opt(&root), json),
        Commands::EnsureRust {
            root,
            scaffold,
            json,
        } => run_ensure_rust(path_opt(&root), scaffold, json),
        Commands::EnsureJava { root, json } => run_ensure_java(path_opt(&root), json),
        Commands::EnsureC { root, json } => run_ensure_c(path_opt(&root), json),
        Commands::EnsureVerus {
            root,
            check_only,
            json,
        } => run_ensure_verus(path_opt(&root), !check_only, json),
        Commands::EnsureDafny {
            root,
            check_only,
            json,
        } => run_ensure_dafny(path_opt(&root), !check_only, json),
        Commands::EnsureLsc {
            root,
            check_only,
            json,
        } => run_ensure_lsc(path_opt(&root), !check_only, json),
        Commands::EnsureOpenjml {
            root: _,
            path,
            check_only,
            json,
        } => run_ensure_openjml(path.as_deref().map(Path::new), !check_only, json),
        Commands::EnsureFramac {
            root: _,
            path,
            check_only,
            json,
        } => run_ensure_framac(path.as_deref().map(Path::new), !check_only, json),
        Commands::GetWorkflowStatus { root, change } => {
            run_workflow_status(path_opt(&root), change.as_deref())
        }
        Commands::Propose { name, micro } => {
            let cwd = std::env::current_dir().unwrap_or_default();
            run_propose(&cwd, &name, micro)
        }
        Commands::List { root, json } => {
            let cwd = path_opt(&root)
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            run_list(&cwd, json)
        }
        Commands::Status { root, change, json } => {
            run_status(path_opt(&root), change.as_deref(), json)
        }
        Commands::Instructions {
            artifact,
            change,
            root,
            plain,
        } => run_instructions(&artifact, &change, path_opt(&root), !plain),
        Commands::Archive {
            name,
            root,
            skip_check,
            skip_specs,
            ack,
        } => run_archive(ArchiveOptions {
            name: &name,
            root: path_opt(&root),
            skip_check,
            skip_specs,
            waiver_ack: ack.as_deref(),
        }),
        Commands::Waive {
            change,
            ack,
            root,
            json,
        } => run_waive(WaiveOptions {
            root: path_opt(&root),
            change: &change,
            ack: &ack,
            json,
        }),
        Commands::Sync {
            change,
            root,
            dry_run,
            specs_only,
            annotations_only,
        } => run_sync(SyncOptions {
            change: &change,
            root: path_opt(&root),
            dry_run,
            specs_only,
            annotations_only,
        }),
        Commands::Check {
            root,
            change,
            strict,
            json,
            pbt_seed,
            pbt_examples,
        } => run_check(CheckOptions {
            root: path_opt(&root),
            change: change.as_deref(),
            strict,
            json,
            quiet: false,
            pbt_seed,
            pbt_examples,
        }),
        Commands::Export { command } => match command {
            ExportCommands::Rtm { root, change, out } => {
                crate::commands::export_rtm::run_export_rtm(
                    path_opt(&root),
                    change.as_deref(),
                    out.as_deref().map(std::path::Path::new),
                )
            }
        },
        Commands::Assurance { command } => match command {
            AssuranceCommands::Check { root, change } => {
                crate::commands::assurance_check::run_assurance_check(
                    path_opt(&root),
                    change.as_deref(),
                )
            }
            AssuranceCommands::Waive {
                root,
                requirement,
                expires_in_days,
                reason,
            } => crate::commands::assurance_check::run_assurance_waive(
                path_opt(&root),
                &requirement,
                expires_in_days,
                &reason,
            ),
        },
        Commands::Mapping { command } => match command {
            MappingCommands::Validate { root } => run_mapping_validate(path_opt(&root)),
            MappingCommands::Migrate { root } => {
                crate::commands::mapping_migrate::run_mapping_migrate(path_opt(&root))
            }
            MappingCommands::Import {
                root,
                dry_run,
                json,
            } => run_mapping_import_command(
                crate::commands::mapping_import::MappingImportCommandOptions {
                    root: path_opt(&root),
                    dry_run,
                    json,
                },
            ),
            MappingCommands::Discover {
                root,
                dry_run,
                json,
            } => run_mapping_discover_command(
                crate::commands::mapping_discover::MappingDiscoverCommandOptions {
                    root: path_opt(&root),
                    dry_run,
                    json,
                },
            ),
        },
        Commands::Hook { command } => match command {
            HookCommands::SessionStart => run_hook_session_start(),
            HookCommands::PreToolUse => run_hook_pre_tool_use(),
            HookCommands::PostEdit => run_hook_post_edit(),
            HookCommands::PostToolUseBrowser => run_hook_post_tool_use_browser(),
            HookCommands::Stop => run_hook_stop(),
        },
        Commands::Daemon {
            root,
            change,
            once,
            debounce_ms,
        } => run_daemon(crate::daemon::DaemonOptions {
            root: path_opt(&root),
            change: change.as_deref(),
            once,
            debounce_ms,
        }),
        Commands::Hooks { command } => match command {
            HooksCommands::Install { root, global } => {
                let cwd = path_opt(&root)
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                run_hooks_install(&cwd, global)
            }
            HooksCommands::Print => {
                let cwd = std::env::current_dir().unwrap_or_default();
                run_hooks_print(&cwd)
            }
        },
    }
}

fn read_hook_input() -> serde_json::Value {
    let mut raw = String::new();
    if io::stdin().read_to_string(&mut raw).is_err() || raw.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
}

fn verification_summary_payload(
    state: &str,
    stale_grade: Option<&str>,
    summary: &VerificationSummary,
    critical_issues: usize,
    change: Option<&str>,
    report_path: Option<String>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "state": state,
        "proved": summary.proved_requirements,
        "propertyTested": summary.property_tested_requirements,
        "specTracked": summary.spec_tracked_requirements,
        "totalRequirements": summary.total_requirements,
        "criticalIssues": critical_issues,
    });
    if let Some(grade) = stale_grade {
        payload["staleGrade"] = serde_json::Value::String(grade.to_string());
    }
    if let Some(change) = change {
        payload["change"] = serde_json::Value::String(change.to_string());
    }
    if let Some(report_path) = report_path {
        payload["reportPath"] = serde_json::Value::String(report_path);
    }
    payload
}

fn report_verification_payload(
    root: &Path,
    state: &str,
    requested_change: Option<&str>,
    report: &VerifyReport,
) -> serde_json::Value {
    let change = report.change.as_deref().or(requested_change);
    let stem = change.unwrap_or("project");
    let report_path = verify_reports_dir(root).join(format!("{stem}-verify.json"));
    let report_path = report_path
        .strip_prefix(root)
        .unwrap_or(&report_path)
        .display()
        .to_string();
    let critical_issues = report
        .issues
        .iter()
        .filter(|issue| issue.severity == IssueSeverity::Critical)
        .count();
    let stale_grade = if state == "stale" {
        worst_ledger_grade(root, change)
    } else {
        None
    };
    verification_summary_payload(
        state,
        stale_grade.as_deref(),
        &report.verification_summary,
        critical_issues,
        change,
        Some(report_path),
    )
}

/// The worst staleness grade across the ledger, refining a whole-file
/// "stale" into WHAT drifted. Tool versions are resolved from the ledger's
/// own records, so toolchain drift is deliberately out of scope for this
/// fast path — only a full `forall check` re-probes provers.
fn worst_ledger_grade(root: &Path, change: Option<&str>) -> Option<String> {
    use crate::verify::staleness::{StalenessGrade, grade_entry};
    let ledger = crate::verify::ledger::read_ledger(root, change)?;
    let requirements = crate::mapping::load::requirements_for_check(root, change).ok()?;
    let recorded_versions: std::collections::BTreeMap<String, String> = ledger
        .entries
        .iter()
        .flat_map(|entry| entry.obligations.iter())
        .filter_map(|obligation| {
            obligation
                .tool_version
                .clone()
                .map(|version| (obligation.tool.clone(), version))
        })
        .collect();
    let mut worst: Option<StalenessGrade> = None;
    for entry in &ledger.entries {
        let Some(requirement) = requirements
            .iter()
            .find(|requirement| requirement.id == entry.requirement_id)
        else {
            continue;
        };
        let grade = grade_entry(root, entry, requirement, |tool| {
            recorded_versions.get(tool).cloned()
        });
        let rank = |grade: &StalenessGrade| match grade {
            StalenessGrade::CodeDrift => 4,
            StalenessGrade::SpecDrift => 3,
            StalenessGrade::Unknown => 2,
            StalenessGrade::ToolchainDrift => 1,
            StalenessGrade::Fresh => 0,
        };
        if worst
            .as_ref()
            .is_none_or(|current| rank(&grade) > rank(current))
        {
            worst = Some(grade);
        }
    }
    match worst? {
        StalenessGrade::Fresh => None,
        StalenessGrade::CodeDrift => Some("code_drift".to_string()),
        StalenessGrade::SpecDrift => Some("spec_drift".to_string()),
        StalenessGrade::ToolchainDrift => Some("toolchain_drift".to_string()),
        StalenessGrade::Unknown => Some("unknown".to_string()),
    }
}

fn empty_verification_payload(state: &str, change: Option<&str>) -> serde_json::Value {
    verification_summary_payload(
        state,
        None,
        &VerificationSummary {
            total_requirements: 0,
            proved_requirements: 0,
            property_tested_requirements: 0,
            spec_tracked_requirements: 0,
        },
        0,
        change,
        None,
    )
}

fn run_hook_session_start() -> i32 {
    let input = read_hook_input();
    let root = project_root(&input).or_else(|| {
        let cwd = input.get("cwd").and_then(|v| v.as_str())?;
        let cwd = std::path::Path::new(cwd);
        let ensure = ensure_verified_project(cwd);
        if !ensure.ok {
            return None;
        }
        find_project_root(cwd)
    });

    let Some(root) = root else {
        println!("{{}}");
        return 0;
    };

    let change = resolve_active_change(&root);
    let active = list_active_changes(&root);
    let (apply_ready, apply_blocked_by) = if let Some(ref c) = change {
        if let Ok(schema) = load_workflow_schema(&root) {
            let status = compute_change_status(&root, &schema, c);
            (status.apply_ready, status.apply_blocked_by)
        } else {
            (false, Vec::new())
        }
    } else {
        (false, Vec::new())
    };

    let mut lines = vec![
        "The Forall workflow is **mandatory** in this project (enforced by hooks and base agent instructions).".to_string(),
        "Use native specs tools (`specs.propose`, `specs.status`, `specs.check`, …) for workflow.".to_string(),
        "Default logic to `verified: true` for TypeScript, Rust, and Java — hooks auto-acquire proof toolchains (`forall ensure-lsc`, `forall ensure-verus`, `forall ensure-java`). Never set `verified: false` on requirements that were marked for formal proof to bypass a failing check (proofs, scenarios, intent); fix the issue or ask the user to accept spec-tracked scope.".to_string(),
    ];
    if let Some(ref c) = change {
        lines.push(format!(
            "Active change: `{c}`. Apply {}.",
            if apply_ready {
                "ready".to_string()
            } else {
                format!("blocked ({})", apply_blocked_by.join(", "))
            }
        ));
    } else if !active.is_empty() {
        lines.push(format!(
            "No active change selected. Set FORALL_CHANGE or .forall/workflow/active. Changes: {}",
            active.join(", ")
        ));
    } else {
        lines.push(
            "No active change — for implementation work, derive a kebab-case name from the request and run `specs.propose <name>` (don't ask the user to pick a name)."
                .to_string(),
        );
    }
    lines.push(
        "Stop hook runs `forall check` when a change is active and check inputs changed; skips the check and the verification card on casual turns. Blocks completion on CRITICAL failures."
            .to_string(),
    );
    lines.push(
        "Hot loop after app-code edits: `forall sync --change <name>` then `forall check --change <name>` (for `property_tested` reqs only: `--pbt-seed` / `--pbt-examples` for repro)."
            .to_string(),
    );

    let workflow_status = build_workflow_status(&root, change.as_deref());
    let status_json =
        serde_json::to_string_pretty(&workflow_status).unwrap_or_else(|_| "{}".to_string());
    lines.push(format!(
        "## getWorkflowStatus (machine snapshot)\n```json\n{status_json}\n```"
    ));

    let verification_summary = workflow_status.last_check.as_ref().and_then(|last_check| {
        read_verify_report(&root, last_check.change.as_deref().or(change.as_deref())).map(
            |report| {
                report_verification_payload(
                    &root,
                    if last_check.ok { "fresh" } else { "failed" },
                    last_check.change.as_deref().or(change.as_deref()),
                    &report,
                )
            },
        )
    });
    let mut output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": lines.join("\n"),
        }
    });
    if let Some(verification_summary) = verification_summary {
        output["verificationSummary"] = verification_summary;
    }
    println!(
        "{}",
        serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_string())
    );
    0
}

fn run_hook_pre_tool_use() -> i32 {
    let input = read_hook_input();
    let payload = pre_tool_use_hook_output(&input);
    if payload == "{}" {
        println!("{{}}");
    } else {
        write_pre_tool_use_block(&payload);
    }
    0
}

fn edited_app_paths(input: &serde_json::Value, root: &Path, extensions: &[&str]) -> Vec<String> {
    let tool_name = input
        .get("toolName")
        .or_else(|| input.get("tool_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("apply_patch");
    let carrier = tool_input_from_json(input.clone());
    crate::hooks::paths::extract_edit_paths_from_tool(root, tool_name, &carrier)
        .into_iter()
        .filter(|p| extensions.iter().any(|ext| p.ends_with(ext)))
        .collect()
}

fn edited_typescript_paths(input: &serde_json::Value, root: &Path) -> Vec<String> {
    edited_app_paths(input, root, &[".ts", ".tsx"])
}

fn edited_rust_paths(input: &serde_json::Value, root: &Path) -> Vec<String> {
    edited_app_paths(input, root, &[".rs"])
}

fn edited_java_paths(input: &serde_json::Value, root: &Path) -> Vec<String> {
    edited_app_paths(input, root, &[".java"])
}

fn edited_c_paths(input: &serde_json::Value, root: &Path) -> Vec<String> {
    edited_app_paths(input, root, &[".c"])
}

fn run_hook_post_edit() -> i32 {
    let input = read_hook_input();
    let Some(root) = project_root(&input) else {
        return 0;
    };

    let tool_name = input
        .get("toolName")
        .or_else(|| input.get("tool_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !tool_name.is_empty()
        && !regex::Regex::new(r"(?i)write|edit|apply_patch|patch")
            .unwrap_or_else(|err| panic!("invalid edit-tool regex: {err}"))
            .is_match(tool_name)
    {
        return 0;
    }

    let ts_files = edited_typescript_paths(&input, &root);
    let rs_files = edited_rust_paths(&input, &root);
    let java_files = edited_java_paths(&input, &root);
    let c_files = edited_c_paths(&input, &root);
    if ts_files.is_empty() && rs_files.is_empty() && java_files.is_empty() && c_files.is_empty() {
        return 0;
    }

    let selected_change = resolve_active_change(&root);
    let cached_report = read_verify_report(&root, selected_change.as_deref());
    let verification_summary = cached_report
        .as_ref()
        .map(|report| {
            report_verification_payload(&root, "stale", selected_change.as_deref(), report)
        })
        .unwrap_or_else(|| empty_verification_payload("stale", selected_change.as_deref()));

    let Some(change) = selected_change else {
        eprintln!("forall hook post-edit: skipped sync (no active change; set FORALL_CHANGE)");
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "verificationSummary": verification_summary,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
        return 0;
    };

    let mut message = String::new();
    if !ts_files.is_empty() {
        let lsc_ready = ensure_lsc_for_agent();
        let code = run_sync(SyncOptions {
            change: &change,
            root: Some(&root),
            dry_run: false,
            specs_only: false,
            annotations_only: true,
        });
        message = if code == 0 {
            let mut msg = format!(
                "Forall synced annotations for change `{change}` after editing: {}.",
                ts_files.join(", ")
            );
            if !lsc_ready {
                msg.push_str(
                    " LemmaScript was not available — hooks attempted `forall ensure-lsc`.",
                );
            }
            msg
        } else {
            format!(
                "Forall sync failed for change `{change}`. Run `forall sync --change {change}`."
            )
        };
    }
    if !rs_files.is_empty() {
        let rust_setup = ensure_rust_verification(&root, true);
        let verus_ready = ensure_verus_for_agent();
        if !message.is_empty() {
            message.push(' ');
        }
        let mut parts = vec![format!(
            "Verus specs live in source — no sync for: {}.",
            rs_files.join(", ")
        )];
        if rust_setup.config_updated {
            parts.push("Registered rust/verus adapter defaults.".to_string());
        }
        if !rust_setup.cargo_updated.is_empty() {
            parts.push(format!(
                "Added Verus metadata to {}.",
                rust_setup.cargo_updated.join(", ")
            ));
        }
        if rust_setup.scaffolded {
            parts.push("Scaffolded Verus Cargo.toml sample.".to_string());
        }
        if !verus_ready {
            parts.push(
                "Verus toolchain not available — hooks attempted `forall ensure-verus`; run it manually if proofs fail.".to_string(),
            );
        }
        parts.push(format!(
            "Run `forall check --change {change}` next (hot loop)."
        ));
        message.push_str(&parts.join(" "));
    } else if !java_files.is_empty() {
        let java_setup = ensure_java_for_agent(&root);
        if !message.is_empty() {
            message.push(' ');
        }
        let mut parts = vec![format!(
            "JML specs live in source — no sync for: {}.",
            java_files.join(", ")
        )];
        if java_setup.config_updated {
            parts.push("Registered java/openjml adapter defaults.".to_string());
        }
        if !java_setup.openjml_ready {
            parts.push(
                "OpenJML toolchain not available — hooks attempted `forall ensure-java`; run it manually if proofs fail.".to_string(),
            );
        }
        parts.push(format!(
            "Run `forall check --change {change}` next (hot loop)."
        ));
        message.push_str(&parts.join(" "));
    } else if !c_files.is_empty() {
        let c_setup = ensure_c_for_agent(&root);
        if !message.is_empty() {
            message.push(' ');
        }
        let mut parts = vec![format!(
            "ACSL specs live in source — no sync for: {}.",
            c_files.join(", ")
        )];
        if c_setup.config_updated {
            parts.push("Registered c/framac adapter defaults.".to_string());
        }
        if !c_setup.framac_ready {
            parts.push(
                "Frama-C toolchain not available — hooks attempted `forall ensure-c`; run it manually if proofs fail.".to_string(),
            );
        }
        parts.push(format!(
            "Run `forall check --change {change}` next (hot loop)."
        ));
        message.push_str(&parts.join(" "));
    } else if !message.is_empty() {
        message.push_str(&format!(
            " Run `forall check --change {change}` next (hot loop)."
        ));
    }

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "verificationSummary": verification_summary,
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": message,
            }
        }))
        .unwrap_or_else(|_| "{}".to_string())
    );
    0
}

fn hook_session_id(input: &serde_json::Value) -> Option<String> {
    input
        .get("session_id")
        .or_else(|| input.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn hook_transcript_path(input: &serde_json::Value) -> Option<String> {
    input
        .get("transcript_path")
        .or_else(|| input.get("transcriptPath"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn hook_last_assistant_message(input: &serde_json::Value) -> Option<String> {
    input
        .get("last_assistant_message")
        .or_else(|| input.get("lastAssistantMessage"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn run_hook_post_tool_use_browser() -> i32 {
    let input = read_hook_input();
    let Some(root) = project_root(&input) else {
        println!("{{}}");
        return 0;
    };
    let tool_name = input
        .get("toolName")
        .or_else(|| input.get("tool_name"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if let Some(session_id) = hook_session_id(&input) {
        record_browser_evidence(&root, &session_id, tool_name);
    }
    println!("{{}}");
    0
}

/// True when the cached verify report is still valid for the Stop hook, so
/// `forall check` can be skipped entirely. Only skips when the last report for
/// this change passed *and* none of its report-relevant inputs (mapping,
/// change delta, verified files) changed since — any edit (e.g. from
/// `post_tool_use`) changes a file's mtime/size and invalidates the fingerprint,
/// forcing a re-run.
fn should_skip_stop_check(
    root: &Path,
    change: Option<&str>,
    cached_report: Option<&VerifyReport>,
) -> bool {
    cached_report.is_some_and(|report| {
        report.ok && check_inputs_unchanged(root, change, &report.verified_files)
    })
}

fn run_hook_stop() -> i32 {
    let input = read_hook_input();
    let Some(root) = project_root(&input) else {
        println!("{{}}");
        return 0;
    };

    if let Some(reason) = validate_browser_verification_claims(&BrowserClaimInput {
        last_assistant_message: hook_last_assistant_message(&input).as_deref(),
        session_id: hook_session_id(&input).as_deref(),
        transcript_path: hook_transcript_path(&input).as_deref(),
        project_root: Some(&root),
    }) {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "decision": "block",
                "reason": reason,
            }))
            .unwrap_or_else(|_| "{}".to_string())
        );
        return 0;
    }

    let stop_plan = plan_stop_check(&root);
    if !stop_plan.run {
        println!("{{}}");
        return 0;
    }

    let change = stop_plan.change.as_deref();
    let report_path = verify_reports_dir(&root);

    let cached_report = read_verify_report(&root, change);
    let skip_check = should_skip_stop_check(&root, change, cached_report.as_ref());

    let (code, report) = if skip_check {
        (0, cached_report)
    } else {
        let code = run_check(CheckOptions {
            root: Some(&root),
            change,
            strict: false,
            json: false,
            quiet: true,
            pbt_seed: 42,
            pbt_examples: 100,
        });
        (code, read_verify_report(&root, change))
    };
    let tier = report
        .as_ref()
        .map(|r| format_verification_tier(&r.verification_summary));

    if code == 0 {
        if let Some(reason) = validate_tier_honesty_claims(&TierClaimInput {
            last_assistant_message: hook_last_assistant_message(&input).as_deref(),
            summary: report.as_ref().map(|r| &r.verification_summary),
        }) {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "decision": "block",
                    "reason": reason,
                }))
                .unwrap_or_else(|_| "{}".to_string())
            );
            return 0;
        }

        // Casual turns (chat-only) leave check inputs unchanged, so the Stop hook
        // skips re-running `forall check`. Do not surface a verification card for
        // that cache hit — it looks like the reply itself was verified.
        if skip_check {
            println!("{{}}");
            return 0;
        }

        let context = report
            .as_ref()
            .and_then(|r| build_stop_check_success_message(change?, r, tier.as_deref()));
        if let Some(report) = report.as_ref() {
            let mut output = serde_json::json!({
                "verificationSummary": report_verification_payload(
                    &root,
                    "fresh",
                    change,
                    report,
                ),
            });
            if let Some(context) = context {
                output["systemMessage"] = serde_json::Value::String(context);
            }
            println!(
                "{}",
                serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_string())
            );
        } else {
            println!("{{}}");
        }
        return 0;
    }

    let tier_suffix = tier
        .as_ref()
        .map(|t| format!(" Verification tier: {t}."))
        .unwrap_or_default();
    let change_suffix = change
        .map(|c| format!(" for change '{c}'"))
        .unwrap_or_default();
    let reason = format!(
        "forall check failed{change_suffix}.{tier_suffix} Fix CRITICAL issues before finishing the turn. See {}/",
        report_path
            .strip_prefix(&root)
            .unwrap_or(&report_path)
            .display()
    );
    let verification_summary = report
        .as_ref()
        .map(|report| report_verification_payload(&root, "failed", change, report))
        .unwrap_or_else(|| empty_verification_payload("failed", change));
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "decision": "block",
            "reason": reason,
            "verificationSummary": verification_summary,
        }))
        .unwrap_or_else(|_| "{}".to_string())
    );
    0
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{empty_verification_payload, report_verification_payload, should_skip_stop_check};
    use crate::mapping::schema::{IssueSeverity, VerificationSummary, VerifyIssue, VerifyReport};

    fn report(ok: bool) -> VerifyReport {
        VerifyReport {
            ok,
            strict: false,
            root: "/tmp/project".to_string(),
            change: Some("add-clamp".to_string()),
            phases: BTreeMap::new(),
            issues: if ok {
                Vec::new()
            } else {
                vec![
                    VerifyIssue {
                        severity: IssueSeverity::Critical,
                        phase: "proofs".to_string(),
                        requirement_id: Some("clamp-bounds".to_string()),
                        message: "proof failed".to_string(),
                        file: None,
                        counterexample: None,
                        pbt: None,
                        proof_detail: None,
                    },
                    VerifyIssue {
                        severity: IssueSeverity::Critical,
                        phase: "mapping".to_string(),
                        requirement_id: None,
                        message: "mapping failed".to_string(),
                        file: None,
                        counterexample: None,
                        pbt: None,
                        proof_detail: None,
                    },
                ]
            },
            verified_files: Vec::new(),
            verification_summary: VerificationSummary {
                total_requirements: 6,
                proved_requirements: 2,
                property_tested_requirements: 1,
                spec_tracked_requirements: 3,
            },
        }
    }

    #[test]
    fn fresh_payload_uses_canonical_report_path() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            report_verification_payload(root.path(), "fresh", Some("add-clamp"), &report(true)),
            json!({
                "state": "fresh",
                "proved": 2,
                "propertyTested": 1,
                "specTracked": 3,
                "totalRequirements": 6,
                "criticalIssues": 0,
                "change": "add-clamp",
                "reportPath": ".forall/verify/cache/reports/add-clamp-verify.json",
            })
        );
    }

    #[test]
    fn stale_payload_supports_edits_without_an_active_change() {
        assert_eq!(
            empty_verification_payload("stale", None),
            json!({
                "state": "stale",
                "proved": 0,
                "propertyTested": 0,
                "specTracked": 0,
                "totalRequirements": 0,
                "criticalIssues": 0,
            })
        );
    }

    #[test]
    fn failed_payload_counts_critical_issues() {
        let root = tempfile::tempdir().expect("tempdir");
        let payload =
            report_verification_payload(root.path(), "failed", Some("add-clamp"), &report(false));
        assert_eq!(payload["state"], "failed");
        assert_eq!(payload["criticalIssues"], 2);
        assert_eq!(payload["totalRequirements"], 6);
    }

    /// Sets up a project with a mapping file and a check-inputs fingerprint
    /// matching a prior successful check over `verified_files`, mirroring what
    /// `write_report_artifacts` persists after a passing `forall check`.
    fn setup_project_with_fingerprint(
        verified_files: &[&str],
    ) -> (tempfile::TempDir, VerifyReport) {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::paths::ensure_forall_layout(dir.path()).expect("layout");
        std::fs::write(
            crate::paths::verify_mapping_path(dir.path()),
            "version: 1\nrequirements: []\n",
        )
        .expect("mapping");
        for file in verified_files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("parent dir");
            }
            std::fs::write(&path, "// verified\n").expect("verified file");
        }

        let mut cached = report(true);
        cached.verified_files = verified_files
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        crate::verify::check_inputs::write_check_inputs_fingerprint(
            dir.path(),
            cached.change.as_deref(),
            &cached.verified_files,
        )
        .expect("fingerprint");

        (dir, cached)
    }

    #[test]
    fn skips_check_when_report_ok_and_inputs_unchanged() {
        let (dir, cached) = setup_project_with_fingerprint(&["src/lib.rs"]);
        assert!(should_skip_stop_check(
            dir.path(),
            cached.change.as_deref(),
            Some(&cached)
        ));
    }

    #[test]
    fn reruns_check_when_cached_report_failed() {
        let (dir, mut cached) = setup_project_with_fingerprint(&["src/lib.rs"]);
        cached.ok = false;
        assert!(!should_skip_stop_check(
            dir.path(),
            cached.change.as_deref(),
            Some(&cached)
        ));
    }

    #[test]
    fn reruns_check_when_no_cached_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!should_skip_stop_check(dir.path(), Some("add-clamp"), None));
    }

    #[test]
    fn reruns_check_when_verified_file_edited_after_fingerprint() {
        let (dir, cached) = setup_project_with_fingerprint(&["src/lib.rs"]);
        // Simulate a post_tool_use edit: content length changes, so the
        // mtime+size fingerprint token no longer matches, even if the mtime
        // second-resolution happens to collide.
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "// edited by post_tool_use, now a different length\n",
        )
        .expect("edit verified file");

        assert!(!should_skip_stop_check(
            dir.path(),
            cached.change.as_deref(),
            Some(&cached)
        ));
    }

    #[test]
    fn reruns_check_when_mapping_changes_after_fingerprint() {
        let (dir, cached) = setup_project_with_fingerprint(&["src/lib.rs"]);
        std::fs::write(
            crate::paths::verify_mapping_path(dir.path()),
            "version: 1\nrequirements:\n  - id: new-req\n",
        )
        .expect("mapping edit");

        assert!(!should_skip_stop_check(
            dir.path(),
            cached.change.as_deref(),
            Some(&cached)
        ));
    }
}
