use std::fs;
use std::path::Path;
use std::process::Command;

use crate::active_change::write_active_change_file;
use crate::bootstrap::bootstrap_forall_integration;
use crate::paths::{
    FORALL_DIR, ensure_dir, ensure_forall_layout, find_project_root, migrate_legacy_layout,
    prune_stale_forall_artifacts, verify_dir, verify_mapping_path, workflow_config_path,
    write_if_missing,
};
use crate::verify::adapters::{
    ensure_c_adapter_persisted, ensure_java_adapter_persisted, ensure_rust_adapter_persisted,
};
use crate::verify::backends::framac_install::{
    ensure_framac_toolchain, link_framac_from, project_has_c_sources,
};
use crate::verify::backends::lsc_install::{ensure_lsc_toolchain, project_has_typescript_sources};
use crate::verify::backends::openjml_install::{
    ensure_openjml_toolchain, link_openjml_from, project_has_java_sources,
};
use crate::verify::backends::verus::{
    ensure_cargo_verus_metadata, find_cargo_toml_files, project_has_rust_sources,
};
use crate::verify::backends::verus_install::ensure_verus_toolchain;
use crate::workflow::change_class::{ChangeClass, write_change_meta};
use crate::workflow::paths::{change_dir, list_active_changes};
use crate::workflow::schema::read_template;

const CONFIG: &str = "schema: forall
context: |
  This project uses the Forall workflow. Write specs before code.
  Run `forall check` before archive.
rules:
  verification:
    - Run `forall check --change <name>` and fix all CRITICAL issues before archive
";

const MAPPING: &str = "version: 1
requirements: []
";

fn init_user_message(quiet: bool, msg: impl AsRef<str>) {
    if !quiet {
        println!("{}", msg.as_ref());
    }
}

fn init_user_error(quiet: bool, msg: impl AsRef<str>) {
    if !quiet {
        eprintln!("{}", msg.as_ref());
    }
}

pub fn run_init(cwd: &Path, verus_scaffold: bool, quiet: bool, agents_md: bool) -> i32 {
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let forall_dir = root.join(FORALL_DIR);

    if let Err(err) = ensure_forall_layout(&root) {
        init_user_error(
            quiet,
            format!("forall init: failed to create layout: {err}"),
        );
        return 1;
    }

    if let Ok(migrated) = migrate_legacy_layout(&root)
        && !quiet
        && !migrated.is_empty()
    {
        init_user_message(
            quiet,
            "forall init: migrated legacy .forall layout to Option B",
        );
        for line in migrated {
            init_user_message(quiet, format!("  {line}"));
        }
    }

    if let Ok(pruned) = prune_stale_forall_artifacts(&root)
        && !quiet
        && !pruned.is_empty()
    {
        init_user_message(quiet, "forall init: removed stale .forall artifacts");
        for line in pruned {
            init_user_message(quiet, format!("  {line}"));
        }
    }

    let _ = write_if_missing(&workflow_config_path(&root), CONFIG);
    let _ = write_if_missing(&verify_mapping_path(&root), MAPPING);

    if let Err(err) = bootstrap_forall_integration(&root, quiet, agents_md) {
        init_user_error(quiet, format!("forall init: bootstrap failed: {err}"));
        return 1;
    }

    if verus_scaffold && let Err(err) = scaffold_verus_crate(&root) {
        init_user_error(quiet, format!("forall init: verus scaffold failed: {err}"));
        return 1;
    }

    init_user_message(
        quiet,
        format!(
            "forall init: created {}/",
            forall_dir
                .strip_prefix(&root)
                .unwrap_or(&forall_dir)
                .display()
        ),
    );
    init_user_message(quiet, "  .forall/workflow/config.yaml");
    init_user_message(quiet, "  .forall/verify/mapping.yaml");
    if verus_scaffold {
        init_user_message(quiet, "  Cargo.toml + src/lib.rs (Verus sample crate)");
        init_user_message(quiet, "  .forall/verify/mapping.sample.yaml");
    }
    for line in crate::bootstrap::bootstrap_message(&root) {
        init_user_message(quiet, format!("  {line}"));
    }
    0
}

pub struct EnsureResult {
    pub ok: bool,
    pub project_root: String,
    pub created: bool,
}

pub fn ensure_verified_project(cwd: &Path) -> EnsureResult {
    let resolved = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    if let Some(existing) = find_project_root(&resolved) {
        return EnsureResult {
            ok: true,
            project_root: existing.display().to_string(),
            created: false,
        };
    }

    let code = run_init(&resolved, false, true, false);
    if code != 0 {
        return EnsureResult {
            ok: false,
            project_root: resolved.display().to_string(),
            created: false,
        };
    }

    let root = find_project_root(&resolved)
        .unwrap_or(resolved)
        .display()
        .to_string();
    EnsureResult {
        ok: true,
        project_root: root,
        created: true,
    }
}

pub struct EnsureRustResult {
    pub ok: bool,
    pub config_updated: bool,
    pub cargo_updated: Vec<String>,
    pub scaffolded: bool,
}

pub fn ensure_rust_verification(root: &Path, scaffold_if_empty: bool) -> EnsureRustResult {
    let mut result = EnsureRustResult {
        ok: true,
        config_updated: false,
        cargo_updated: Vec::new(),
        scaffolded: false,
    };

    if ensure_rust_adapter_persisted(root).unwrap_or(false) {
        result.config_updated = true;
    }

    let has_rust = project_has_rust_sources(root);
    if !has_rust && !scaffold_if_empty {
        return result;
    }

    for cargo in find_cargo_toml_files(root) {
        if ensure_cargo_verus_metadata(&cargo).unwrap_or(false) {
            result.cargo_updated.push(
                cargo
                    .strip_prefix(root)
                    .unwrap_or(&cargo)
                    .display()
                    .to_string(),
            );
        }
    }

    if scaffold_if_empty && !root.join("Cargo.toml").is_file() && has_rust {
        if scaffold_verus_crate(root).is_ok() {
            result.scaffolded = true;
            if ensure_cargo_verus_metadata(&root.join("Cargo.toml")).unwrap_or(false) {
                result.cargo_updated.push("Cargo.toml".to_string());
            }
        } else {
            result.ok = false;
        }
    }

    result
}

pub fn run_ensure_rust(root: Option<&Path>, scaffold: bool, json: bool) -> i32 {
    let cwd = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(project_root) = find_project_root(&cwd) else {
        eprintln!("forall ensure-rust: project not found (run `forall init` first)");
        return 1;
    };

    let result = ensure_rust_verification(&project_root, scaffold);
    if !result.ok {
        eprintln!("forall ensure-rust: setup failed");
        return 1;
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "projectRoot": project_root.display().to_string(),
                "configUpdated": result.config_updated,
                "cargoUpdated": result.cargo_updated,
                "scaffolded": result.scaffolded,
            }))
            .unwrap_or_default()
        );
    } else {
        let mut actions: Vec<String> = Vec::new();
        if result.config_updated {
            actions.push("registered rust/verus adapter defaults".to_string());
        }
        for path in &result.cargo_updated {
            actions.push(format!("added Verus metadata to {path}"));
        }
        if result.scaffolded {
            actions.push("scaffolded Verus Cargo.toml + src/lib.rs sample".to_string());
        }
        if actions.is_empty() {
            println!(
                "forall ensure-rust: Rust/Verus already configured at {}",
                project_root.display()
            );
        } else {
            println!(
                "forall ensure-rust: configured Rust/Verus at {} ({})",
                project_root.display(),
                actions.join("; ")
            );
        }
    }
    0
}

pub fn run_ensure_verus(_root: Option<&Path>, allow_download: bool, json: bool) -> i32 {
    let result = ensure_verus_toolchain(allow_download, false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "alreadyInstalled": result.already_installed,
                "installed": result.installed,
                "version": result.version,
                "toolchainInstalled": result.toolchain_installed,
                "wrapperPath": result.wrapper_path.as_ref().map(|p| p.display().to_string()),
                "message": result.message,
            }))
            .unwrap_or_default()
        );
    } else if result.already_installed {
        println!(
            "forall ensure-verus: already available ({})",
            result
                .version
                .unwrap_or_else(|| "unknown version".to_string())
        );
    } else if result.ok && result.installed {
        println!("forall ensure-verus: {}", result.message);
        if result.toolchain_installed {
            println!("forall ensure-verus: installed required rustup toolchain");
        }
        if let Some(wrapper) = &result.wrapper_path {
            println!("forall ensure-verus: wrapper at {}", wrapper.display());
        }
    } else {
        eprintln!("forall ensure-verus: {}", result.message);
        return 1;
    }
    if result.ok { 0 } else { 1 }
}

/// Best-effort Verus install for agent hooks (quiet, allows download).
pub fn ensure_verus_for_agent() -> bool {
    ensure_verus_toolchain(true, true).ok
}

pub fn run_ensure_dafny(_root: Option<&Path>, allow_download: bool, json: bool) -> i32 {
    let result = crate::verify::backends::dafny_install::ensure_dafny_toolchain(
        allow_download,
        false,
    );
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "alreadyInstalled": result.already_installed,
                "installed": result.installed,
                "version": result.version,
                "binaryPath": result.binary_path.as_ref().map(|p| p.display().to_string()),
                "message": result.message,
            }))
            .unwrap_or_default()
        );
    } else if result.already_installed {
        println!(
            "forall ensure-dafny: already available ({})",
            result
                .version
                .unwrap_or_else(|| "unknown version".to_string())
        );
    } else if result.ok && result.installed {
        println!("forall ensure-dafny: {}", result.message);
        if let Some(binary) = &result.binary_path {
            println!("forall ensure-dafny: binary at {}", binary.display());
        }
    } else {
        eprintln!("forall ensure-dafny: {}", result.message);
        return 1;
    }
    if result.ok { 0 } else { 1 }
}

pub fn run_ensure_lsc(_root: Option<&Path>, allow_download: bool, json: bool) -> i32 {
    let result = ensure_lsc_toolchain(allow_download, false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "alreadyInstalled": result.already_installed,
                "installed": result.installed,
                "version": result.version,
                "scriptPath": result.script_path.as_ref().map(|p| p.display().to_string()),
                "wrapperPath": result.wrapper_path.as_ref().map(|p| p.display().to_string()),
                "message": result.message,
            }))
            .unwrap_or_default()
        );
    } else if result.already_installed {
        println!(
            "forall ensure-lsc: already available ({})",
            result
                .version
                .unwrap_or_else(|| "unknown version".to_string())
        );
    } else if result.ok && result.installed {
        println!("forall ensure-lsc: {}", result.message);
        if let Some(script) = &result.script_path {
            println!("forall ensure-lsc: lsc.js at {}", script.display());
        }
        if let Some(wrapper) = &result.wrapper_path {
            println!("forall ensure-lsc: wrapper at {}", wrapper.display());
        }
    } else {
        eprintln!("forall ensure-lsc: {}", result.message);
        return 1;
    }
    if result.ok { 0 } else { 1 }
}

/// Best-effort LemmaScript install for agent hooks (quiet, allows download).
pub fn ensure_lsc_for_agent() -> bool {
    ensure_lsc_toolchain(true, true).ok
}

pub fn run_ensure_openjml(path: Option<&Path>, allow_install: bool, json: bool) -> i32 {
    if let Some(source) = path
        && let Err(err) = link_openjml_from(source)
    {
        eprintln!(
            "forall ensure-openjml: failed to link from {}: {err}",
            source.display()
        );
        return 1;
    }

    let result = ensure_openjml_toolchain(allow_install, false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "alreadyInstalled": result.already_installed,
                "installed": result.installed,
                "version": result.version,
                "wrapperPath": result.wrapper_path.as_ref().map(|p| p.display().to_string()),
                "installRoot": result.install_root.as_ref().map(|p| p.display().to_string()),
                "message": result.message,
            }))
            .unwrap_or_default()
        );
    } else if result.already_installed {
        println!(
            "forall ensure-openjml: already available ({})",
            result
                .version
                .unwrap_or_else(|| "unknown version".to_string())
        );
    } else if result.ok {
        println!("forall ensure-openjml: {}", result.message);
        if let Some(wrapper) = &result.wrapper_path {
            println!("forall ensure-openjml: wrapper at {}", wrapper.display());
        }
    } else {
        eprintln!("forall ensure-openjml: {}", result.message);
        return 1;
    }
    if result.ok { 0 } else { 1 }
}

/// Best-effort OpenJML provisioning for agent hooks (quiet, links from
/// `FORALL_OPENJML_HOME` or installs the release, as Verus and LemmaScript do).
pub fn ensure_openjml_for_agent() -> bool {
    ensure_openjml_toolchain(true, true).ok
}

pub fn run_ensure_framac(path: Option<&Path>, allow_link: bool, json: bool) -> i32 {
    if let Some(source) = path
        && let Err(err) = link_framac_from(source)
    {
        eprintln!(
            "forall ensure-framac: failed to link from {}: {err}",
            source.display()
        );
        return 1;
    }

    let result = ensure_framac_toolchain(allow_link, false);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "alreadyInstalled": result.already_installed,
                "installed": result.installed,
                "version": result.version,
                "wrapperPath": result.wrapper_path.as_ref().map(|p| p.display().to_string()),
                "message": result.message,
            }))
            .unwrap_or_default()
        );
    } else if result.already_installed {
        println!(
            "forall ensure-framac: already available ({})",
            result
                .version
                .unwrap_or_else(|| "unknown version".to_string())
        );
    } else if result.ok {
        println!("forall ensure-framac: {}", result.message);
        if let Some(wrapper) = &result.wrapper_path {
            println!("forall ensure-framac: wrapper at {}", wrapper.display());
        }
    } else {
        eprintln!("forall ensure-framac: {}", result.message);
        return 1;
    }
    if result.ok { 0 } else { 1 }
}

pub fn ensure_java_verification(root: &Path) -> bool {
    ensure_java_adapter_persisted(root).unwrap_or(false)
}

#[derive(Debug, Clone, Copy)]
pub struct EnsureJavaHookResult {
    pub config_updated: bool,
    pub openjml_ready: bool,
}

/// Register the java/openjml adapter and link OpenJML (best-effort, for hooks).
pub fn ensure_java_for_agent(root: &Path) -> EnsureJavaHookResult {
    EnsureJavaHookResult {
        config_updated: ensure_java_verification(root),
        openjml_ready: ensure_openjml_for_agent(),
    }
}

pub fn run_ensure_java(root: Option<&Path>, json: bool) -> i32 {
    let cwd = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(project_root) = find_project_root(&cwd) else {
        eprintln!("forall ensure-java: project not found (run `forall init` first)");
        return 1;
    };

    let config_updated = ensure_java_verification(&project_root);
    let openjml = ensure_openjml_toolchain(true, false);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "projectRoot": project_root.display().to_string(),
                "configUpdated": config_updated,
                "openjml": {
                    "ok": openjml.ok,
                    "alreadyInstalled": openjml.already_installed,
                    "installed": openjml.installed,
                    "version": openjml.version,
                    "wrapperPath": openjml.wrapper_path.as_ref().map(|p| p.display().to_string()),
                    "installRoot": openjml.install_root.as_ref().map(|p| p.display().to_string()),
                    "message": openjml.message,
                },
            }))
            .unwrap_or_default()
        );
    } else {
        let mut actions: Vec<String> = Vec::new();
        if config_updated {
            actions.push("registered java/openjml adapter defaults".to_string());
        }
        if openjml.ok {
            if openjml.already_installed {
                actions.push(format!(
                    "OpenJML already available ({})",
                    openjml
                        .version
                        .unwrap_or_else(|| "unknown version".to_string())
                ));
            } else {
                actions.push(openjml.message);
            }
        } else {
            actions.push(format!("OpenJML not linked: {}", openjml.message));
        }
        if actions.is_empty() {
            println!(
                "forall ensure-java: Java/OpenJML already configured at {}",
                project_root.display()
            );
        } else {
            println!(
                "forall ensure-java: configured Java/OpenJML at {} ({})",
                project_root.display(),
                actions.join("; ")
            );
        }
    }
    0
}

pub fn ensure_c_verification(root: &Path) -> bool {
    ensure_c_adapter_persisted(root).unwrap_or(false)
}

/// Best-effort Frama-C link for agent hooks (quiet, tries `FORALL_FRAMAC_HOME`).
pub fn ensure_framac_for_agent() -> bool {
    ensure_framac_toolchain(true, true).ok
}

#[derive(Debug, Clone, Copy)]
pub struct EnsureCHookResult {
    pub config_updated: bool,
    pub framac_ready: bool,
}

/// Register the c/framac adapter and link Frama-C (best-effort, for hooks).
pub fn ensure_c_for_agent(root: &Path) -> EnsureCHookResult {
    EnsureCHookResult {
        config_updated: ensure_c_verification(root),
        framac_ready: ensure_framac_for_agent(),
    }
}

pub fn run_ensure_c(root: Option<&Path>, json: bool) -> i32 {
    let cwd = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let Some(project_root) = find_project_root(&cwd) else {
        eprintln!("forall ensure-c: project not found (run `forall init` first)");
        return 1;
    };

    let config_updated = ensure_c_verification(&project_root);
    let framac = ensure_framac_toolchain(true, false);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "projectRoot": project_root.display().to_string(),
                "configUpdated": config_updated,
                "framac": {
                    "ok": framac.ok,
                    "alreadyInstalled": framac.already_installed,
                    "installed": framac.installed,
                    "version": framac.version,
                    "wrapperPath": framac.wrapper_path.as_ref().map(|p| p.display().to_string()),
                    "message": framac.message,
                },
            }))
            .unwrap_or_default()
        );
    } else {
        let mut actions: Vec<String> = Vec::new();
        if config_updated {
            actions.push("registered c/framac adapter defaults".to_string());
        }
        if framac.ok {
            if framac.already_installed {
                actions.push(format!(
                    "Frama-C already available ({})",
                    framac
                        .version
                        .unwrap_or_else(|| "unknown version".to_string())
                ));
            } else {
                actions.push(framac.message);
            }
        } else {
            actions.push(format!("Frama-C not linked: {}", framac.message));
        }
        if actions.is_empty() {
            println!(
                "forall ensure-c: C/Frama-C already configured at {}",
                project_root.display()
            );
        } else {
            println!(
                "forall ensure-c: configured C/Frama-C at {} ({})",
                project_root.display(),
                actions.join("; ")
            );
        }
    }
    0
}

pub fn run_ensure(root: Option<&Path>, json: bool) -> i32 {
    let cwd = root
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let result = ensure_verified_project(&cwd);
    if !result.ok {
        eprintln!("forall ensure: failed to initialize project");
        return 1;
    }

    let project_root = find_project_root(&cwd).unwrap_or(cwd);
    let ts_result = if project_has_typescript_sources(&project_root) {
        let _ = ensure_lsc_toolchain(true, true);
        true
    } else {
        false
    };
    let rust_result = if project_has_rust_sources(&project_root) {
        let _ = ensure_verus_toolchain(true, true);
        Some(ensure_rust_verification(&project_root, false))
    } else {
        None
    };
    let java_result = if project_has_java_sources(&project_root) {
        let _ = ensure_openjml_toolchain(true, true);
        Some(ensure_java_verification(&project_root))
    } else {
        None
    };
    let c_result = if project_has_c_sources(&project_root) {
        let _ = ensure_framac_toolchain(true, true);
        Some(ensure_c_verification(&project_root))
    } else {
        None
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": result.ok,
                "projectRoot": result.project_root,
                "created": result.created,
                "typescript": ts_result,
                "rust": rust_result.as_ref().map(|r| serde_json::json!({
                    "configUpdated": r.config_updated,
                    "cargoUpdated": &r.cargo_updated,
                    "scaffolded": r.scaffolded,
                })),
                "java": java_result,
                "c": c_result,
            }))
            .unwrap_or_default()
        );
    } else if result.created {
        println!("forall ensure: initialized {}", result.project_root);
    } else {
        println!(
            "forall ensure: already initialized at {}",
            result.project_root
        );
    }
    if ts_result {
        println!("forall ensure: attempted LemmaScript toolchain setup (see `forall ensure-lsc`)");
    }
    if let Some(rust) = rust_result
        && (rust.config_updated || !rust.cargo_updated.is_empty() || rust.scaffolded)
    {
        println!(
            "forall ensure: auto-configured Rust/Verus (adapter={}, cargo={:?}, scaffold={})",
            rust.config_updated, rust.cargo_updated, rust.scaffolded
        );
    }
    if let Some(java) = java_result {
        if java {
            println!("forall ensure: registered java/openjml adapter defaults");
        }
        println!(
            "forall ensure: attempted OpenJML toolchain setup (see `forall ensure-java` or `forall ensure-openjml`)"
        );
    }
    if let Some(c) = c_result {
        if c {
            println!("forall ensure: registered c/framac adapter defaults");
        }
        println!(
            "forall ensure: attempted Frama-C toolchain setup (see `forall ensure-c` or `forall ensure-framac`)"
        );
    }
    0
}

pub fn run_propose(cwd: &Path, name: &str, micro: bool) -> i32 {
    let root = find_project_root(cwd)
        .unwrap_or_else(|| cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()));
    if !crate::paths::forall_mapping_path(&root).is_file() {
        eprintln!("forall propose: run `forall init` first");
        return 1;
    }

    let valid = regex::Regex::new(r"^[a-z][a-z0-9-]*$")
        .unwrap_or_else(|err| panic!("invalid change-name regex: {err}"));
    if !valid.is_match(name) {
        eprintln!("forall propose: name must be kebab-case (e.g. add-clamp-docs)");
        return 1;
    }

    let dir = change_dir(&root, name);
    if dir.exists() {
        eprintln!("forall propose: change already exists: {name}");
        return 1;
    }

    let _ = ensure_dir(&dir.join("specs"));
    let vars = [("CHANGE_NAME", name), ("CAPABILITY", "capability")];

    let _ = write_if_missing(
        &dir.join("proposal.md"),
        &read_template(&root, "proposal.md", &vars).unwrap_or_default(),
    );
    let _ = write_if_missing(
        &dir.join("mapping.delta.yaml"),
        &read_template(&root, "mapping-stub.yaml", &vars).unwrap_or_default(),
    );

    let class = if micro {
        ChangeClass::Micro
    } else {
        ChangeClass::Standard
    };
    if write_change_meta(&root, name, class).is_err() {
        eprintln!("forall propose: failed to write change.yaml");
        return 1;
    }

    if write_active_change_file(&root, name).is_err() {
        eprintln!("forall propose: failed to write active change");
        return 1;
    }

    println!(
        "forall propose: created change '{name}' ({})",
        class.as_str()
    );
    println!("  {}/", dir.strip_prefix(&root).unwrap_or(&dir).display());
    println!("  Next: fill proposal.md, then run `forall status --change {name}`");
    0
}

pub fn run_list(cwd: &Path, json: bool) -> i32 {
    let Some(root) = find_project_root(cwd) else {
        eprintln!("forall list: project not found");
        return 1;
    };
    let changes = list_active_changes(&root);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "changes": changes }))
                .unwrap_or_default()
        );
        return 0;
    }
    if changes.is_empty() {
        println!("No active changes. Run `forall propose <name>` to start one.");
        return 0;
    }
    println!("Active changes:");
    for name in changes {
        println!("  {name}");
    }
    0
}

const VERUS_CARGO_TOML: &str = r#"[package]
name = "verified-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
vstd = { git = "https://github.com/verus-lang/verus", package = "vstd" }

[package.metadata.verus]
verify = true
"#;

const VERUS_LIB_RS: &str = r#"use vstd::prelude::*;

verus! {

/// Clamp `x` to the inclusive range `[lo, hi]`.
pub fn clamp(x: u64, lo: u64, hi: u64) -> (result: u64)
    requires
        lo <= hi,
    ensures
        lo <= result <= hi,
{
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

} // verus!
"#;

const VERUS_SAMPLE_MAPPING: &str = r#"version: 1
requirements:
  - id: clamp-bounds
    capability: math
    requirement: Clamp returns a value within [lo, hi]
    verified: true
    code:
      file: src/lib.rs
      symbols: [clamp]
    contract: |
      requires lo <= hi
      ensures lo <= result <= hi
"#;

fn scaffold_verus_crate(root: &Path) -> std::io::Result<()> {
    if root.join("Cargo.toml").is_file() {
        return Ok(());
    }

    let cargo_new = Command::new("cargo")
        .args(["verus", "new", "verified-crate", "--vstd"])
        .current_dir(root)
        .output();
    if let Ok(out) = cargo_new
        && out.status.success()
    {
        let crate_root = root.join("verified-crate");
        if crate_root.join("Cargo.toml").is_file() {
            write_verus_sample_mapping(root)?;
            return Ok(());
        }
    }

    fs::create_dir_all(root.join("src"))?;
    write_if_missing(&root.join("Cargo.toml"), VERUS_CARGO_TOML)?;
    write_if_missing(&root.join("src/lib.rs"), VERUS_LIB_RS)?;
    write_verus_sample_mapping(root)?;
    Ok(())
}

fn write_verus_sample_mapping(root: &Path) -> std::io::Result<()> {
    let sample = verify_dir(root).join("mapping.sample.yaml");
    write_if_missing(&sample, VERUS_SAMPLE_MAPPING)
}
