use std::fs;
use std::path::{Path, PathBuf};

use crate::agent_home::{
    resolve_forall_home, resolve_project_agent_dir, trust_project_in_forall_home,
};
use crate::bundled::bundled_file;
use crate::hooks::paths::PRE_TOOL_USE_MATCHER;
use crate::paths::{
    PROJECT_MARKER_REL, agent_markers_path, agents_md_path, ensure_dir, ensure_forall_layout,
};

const FORALL_HOOK_EXE: &str = "forall";

fn hook_command(subcommand: &str) -> String {
    format!("{FORALL_HOOK_EXE} hook {subcommand}")
}

/// Hook definitions installed into `~/.forall/hooks.json` (user home, never the repo).
pub fn forall_hooks_json() -> serde_json::Value {
    serde_json::json!({
        "hooks": {
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": hook_command("session-start"),
                    "timeout": 30,
                    "statusMessage": "forall context"
                }]
            }],
            "PreToolUse": [{
                "matcher": PRE_TOOL_USE_MATCHER,
                "hooks": [{
                    "type": "command",
                    "command": hook_command("pre-tool-use"),
                    "timeout": 30,
                    "statusMessage": "forall gate"
                }]
            }],
            "PostToolUse": [
                {
                    "matcher": "apply_patch|write|edit|Write|Edit|ApplyPatch",
                    "hooks": [{
                        "type": "command",
                        "command": hook_command("post-edit"),
                        "timeout": 120,
                        "statusMessage": "forall sync"
                    }]
                },
                {
                    "matcher": "*",
                    "hooks": [{
                        "type": "command",
                        "command": hook_command("post-tool-use-browser"),
                        "timeout": 15,
                        "statusMessage": "forall browser evidence"
                    }]
                }
            ],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": hook_command("stop"),
                    "timeout": 300,
                    "statusMessage": "forall check"
                }]
            }]
        }
    })
}

pub fn forall_home_hooks_path() -> PathBuf {
    resolve_forall_home().join("hooks.json")
}

/// Merge Forall hook events into `~/.forall/hooks.json`, preserving unrelated entries.
///
/// The read errors here used to be swallowed: `.ok()` on the read, `.ok()` on
/// the parse, and an empty `{"hooks": {}}` substituted for either failure —
/// which was then written back over the file. So any hooks.json that could not
/// be read or parsed was silently replaced by Forall's four events alone,
/// destroying every other event the user had configured (UserPromptSubmit,
/// Notification, SubagentStop, PreCompact) along with any sibling top-level
/// keys. No error, no warning, no backup, and exit 0.
///
/// That needed no concurrency at all — a hand-edited trailing comma was
/// enough. Concurrency made it worse: the write was a truncating `fs::write`,
/// so a second process could read the zero-byte window mid-write, fail to
/// parse it, and persist the empty substitute. Same mechanism that destroyed
/// a config.toml in this repo, on the same bootstrap path, a few lines above
/// where that one was fixed.
pub fn install_hooks_in_forall_home() -> std::io::Result<PathBuf> {
    install_hooks_in_home(&resolve_forall_home())
}

/// The body of [`install_hooks_in_forall_home`], against an explicit home, so
/// it can be tested without setting a process-global environment variable.
pub fn install_hooks_in_home(home: &Path) -> std::io::Result<PathBuf> {
    let target = home.join("hooks.json");
    ensure_dir(home)?;

    let _guard = crate::agent_home::HomeFileLock::acquire(home, crate::agent_home::HOOKS_LOCK);

    let desired = forall_hooks_json();
    let mut existing = if target.is_file() {
        let raw = fs::read_to_string(&target)?;
        serde_json::from_str::<serde_json::Value>(&raw).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{} is not valid JSON ({err}); refusing to overwrite it",
                    target.display()
                ),
            )
        })?
    } else {
        serde_json::json!({ "hooks": {} })
    };

    match existing.get("hooks") {
        Some(serde_json::Value::Object(_)) => {}
        // A missing or null `hooks` is just an empty one to merge into.
        None | Some(serde_json::Value::Null) => existing["hooks"] = serde_json::json!({}),
        // Anything else holds data in a shape we do not understand, and
        // replacing it would be the same silent deletion in miniature.
        Some(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: `hooks` is not an object; refusing to overwrite it",
                    target.display()
                ),
            ));
        }
    }
    let desired_map = desired["hooks"]
        .as_object()
        .unwrap_or_else(|| panic!("forall hooks template is missing a hooks object"));
    let existing_map = existing["hooks"]
        .as_object_mut()
        .unwrap_or_else(|| panic!("hooks.json hooks entry is not an object"));
    for (event, value) in desired_map {
        existing_map.insert(event.clone(), value.clone());
    }

    forall_utils_path::write_atomically(
        &target,
        &format!("{}\n", serde_json::to_string_pretty(&existing)?),
    )?;
    Ok(target)
}

pub fn bootstrap_forall_integration(
    project_root: &Path,
    quiet: bool,
    agents_md: bool,
) -> std::io::Result<()> {
    let dot_forall = resolve_project_agent_dir(project_root);
    ensure_forall_layout(project_root)?;

    // Same rule as the trust write below: refusing to overwrite a file we
    // cannot parse must not block the edit that triggered this bootstrap.
    if let Err(err) = install_hooks_in_forall_home() {
        if err.kind() != std::io::ErrorKind::InvalidData {
            return Err(err);
        }
        if !quiet {
            eprintln!(
                "forall init: could not update ~/.forall/hooks.json ({err}); \
                 project files were created — repair that file to restore the hooks"
            );
        }
    }

    let markers_toml = format!(
        "# Generated by forall init — project markers for the Forall agent.\nproject_root_markers = [\"{PROJECT_MARKER_REL}\"]\n"
    );
    fs::write(agent_markers_path(project_root), markers_toml)?;

    if agents_md {
        let agents_dest = agents_md_path(project_root);
        if !agents_dest.exists() {
            fs::write(agents_dest, bundled_file("templates/AGENTS.md")?.contents())?;
        }
    }

    trust_project_in_forall_home(project_root).or_else(|err| {
        let permission_denied = err.kind() == std::io::ErrorKind::PermissionDenied
            || err.to_string().contains("Operation not permitted");
        // A global config we cannot parse is now refused rather than
        // overwritten, which is right — but it must not take the project down
        // with it. The PreToolUse gate initializes on the first
        // application-code edit, so treating this as fatal blocked every edit
        // in every project until the user repaired a file that the gate's
        // message never named. The project files are written either way; only
        // the trust record is missing.
        let unusable_config = err.kind() == std::io::ErrorKind::InvalidData;
        if permission_denied || unusable_config {
            if !quiet {
                eprintln!(
                    "forall init: could not update ~/.forall/config.toml ({err}); \
                     project files were created — repair that file, or trust the \
                     project outside the sandbox, if hooks need full access"
                );
            }
            Ok(())
        } else {
            Err(err)
        }
    })?;

    if let Ok(installed) = crate::skills_install::install_workflow_skills()
        && !quiet
        && !installed.is_empty()
    {
        eprintln!(
            "forall init: installed workflow skills: {}",
            installed.join(", ")
        );
    }

    let _ = dot_forall;

    Ok(())
}

pub fn bootstrap_message(_project_root: &Path) -> Vec<String> {
    vec![
        ".forall/markers.toml (project markers)".to_string(),
        "~/.forall/hooks.json (SessionStart, PreToolUse, PostToolUse, Stop)".to_string(),
        "~/.forall/config.toml updated (project trusted)".to_string(),
    ]
}

#[cfg(test)]
mod hooks_merge_tests {
    use super::*;

    fn read(home: &Path) -> serde_json::Value {
        serde_json::from_str(&fs::read_to_string(home.join("hooks.json")).expect("hooks"))
            .expect("valid json")
    }

    /// The doc comment promises unrelated entries are preserved. They were
    /// not: any read or parse failure was swallowed and replaced with an
    /// empty hooks object, which was then written back over the file.
    #[test]
    fn a_hooks_file_that_does_not_parse_is_refused_not_replaced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        // A trailing comma — the kind of thing hand-editing produces.
        let broken = "{\"hooks\": {\"UserPromptSubmit\": [],}}";
        fs::write(home.join("hooks.json"), broken).expect("seed");

        let err = install_hooks_in_home(home).expect_err("must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(home.join("hooks.json")).expect("still there"),
            broken,
            "the unreadable file is left exactly as found"
        );
    }

    /// Events Forall does not own, and keys beside `hooks`, must survive.
    #[test]
    fn unrelated_events_and_sibling_keys_survive_the_merge() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(
            home.join("hooks.json"),
            serde_json::json!({
                "hooks": { "UserPromptSubmit": [{"note": "someone else's"}] },
                "statusLine": { "type": "command" }
            })
            .to_string(),
        )
        .expect("seed");

        install_hooks_in_home(home).expect("merge");

        let merged = read(home);
        assert_eq!(
            merged["hooks"]["UserPromptSubmit"][0]["note"], "someone else's",
            "a third-party event was dropped"
        );
        assert_eq!(merged["statusLine"]["type"], "command", "sibling key lost");
        assert!(
            merged["hooks"]["PreToolUse"].is_array(),
            "Forall's own events are still installed"
        );
    }

    /// `hooks` holding something other than an object is data in a shape we
    /// do not understand; replacing it would be the same silent deletion.
    #[test]
    fn a_non_object_hooks_key_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let seed = "{\"hooks\": \"nonsense\"}";
        fs::write(home.join("hooks.json"), seed).expect("seed");

        let err = install_hooks_in_home(home).expect_err("must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(home.join("hooks.json")).expect("read"),
            seed
        );
    }

    /// A null or absent `hooks` is simply an empty one to merge into.
    #[test]
    fn a_null_hooks_key_is_treated_as_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(home.join("hooks.json"), "{\"hooks\": null, \"keep\": 1}").expect("seed");

        install_hooks_in_home(home).expect("merge");

        let merged = read(home);
        assert!(merged["hooks"]["Stop"].is_array());
        assert_eq!(merged["keep"], 1);
    }

    /// Concurrent installs must not lose the file, the way the sibling
    /// config.toml write did before it was locked and made atomic.
    #[test]
    fn concurrent_installs_keep_unrelated_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(
            home.join("hooks.json"),
            serde_json::json!({ "hooks": { "Notification": [{"keep": true}] } }).to_string(),
        )
        .expect("seed");

        std::thread::scope(|scope| {
            for _ in 0..12 {
                scope.spawn(|| {
                    let _ = install_hooks_in_home(home);
                });
            }
        });

        let merged = read(home);
        assert_eq!(merged["hooks"]["Notification"][0]["keep"], true);
        assert!(merged["hooks"]["SessionStart"].is_array());
    }
}
