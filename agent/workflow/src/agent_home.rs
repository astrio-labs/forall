use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use forall_utils_path::write_atomically;
use toml_edit::DocumentMut;

use crate::paths::{FORALL_DIR, LEGACY_FORALL_DIR};

/// User-level Forall home (`~/.forall`, with legacy `~/.codex` fallback).
pub fn resolve_forall_home() -> PathBuf {
    if let Ok(env) = env::var("FORALL_HOME") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(env) = env::var("CODEX_HOME") {
        let trimmed = env.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    let home = env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    let forall_home = home.join(".forall");
    let legacy_home = home.join(".codex");
    if forall_home.is_dir() {
        return forall_home;
    }
    if legacy_home.is_dir() {
        return legacy_home;
    }
    forall_home
}

/// Project-level Forall directory (`.forall`, with legacy `.codex` / `forall` fallback).
pub fn resolve_project_agent_dir(project_root: &Path) -> PathBuf {
    for name in [FORALL_DIR, ".codex", LEGACY_FORALL_DIR] {
        let dir = project_root.join(name);
        if dir.is_dir() {
            return dir;
        }
    }
    project_root.join(FORALL_DIR)
}

/// Record this project as trusted in the user's global `config.toml`.
///
/// Adding one table means rewriting the whole file, which is the entire user
/// config — provider settings, credentials, and every project they have ever
/// trusted. So this must never be a plain read-then-`fs::write`: `fs::write`
/// truncates to zero and *then* writes, so a concurrent reader can observe a
/// half-written file and persist that half as the new truth.
///
/// That is not theoretical. Forty `forall init` runs racing against a
/// 1200-entry config (parallel tests, two terminals, the desktop opening
/// several sessions at once) left three entries in one round and unparseable
/// TOML in another. It destroyed a real config in this repo.
///
/// So: serialize on a lock file, edit the document as TOML instead of
/// concatenating text, and rename the replacement into place. The lock is
/// best-effort — if it cannot be taken we still take the atomic path, which
/// alone rules out the destructive interleavings, because every reader then
/// sees a whole file and the worst case degrades to one lost entry.
pub fn trust_project_in_forall_home(project_root: &Path) -> std::io::Result<()> {
    trust_project_in_home(&resolve_forall_home(), project_root)
}

/// The body of [`trust_project_in_forall_home`], against an explicit home.
///
/// Taking the home as an argument is what lets this be tested at all: the
/// resolved home comes from `$FORALL_HOME`, and environment variables are
/// process-global, so a test that set one would be racing every other test in
/// the same binary.
pub fn trust_project_in_home(home: &Path, project_root: &Path) -> std::io::Result<()> {
    let config_path = home.join("config.toml");
    crate::paths::ensure_dir(home)?;

    let _guard = HomeFileLock::acquire(home, CONFIG_LOCK);

    let existing = match fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let mut doc = existing.parse::<DocumentMut>().map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is not valid TOML ({err}); refusing to overwrite it",
                config_path.display()
            ),
        )
    })?;

    let key = project_root.display().to_string();

    // An exact key lookup, not a substring scan of the whole file. The old
    // check also accepted a bare `contains(&root_str)`, so trusting /a/b was
    // skipped whenever /a/bc was already trusted — or whenever the path
    // happened to appear inside any unrelated value or comment.
    //
    // `as_table_like` covers both spellings: `[projects."/x"]` and the inline
    // `projects = { "/x" = { ... } }`. Reading only the former missed an
    // inline table entirely.
    if doc
        .get("projects")
        .and_then(|projects| projects.as_table_like())
        .is_some_and(|projects| projects.get(&key).is_some())
    {
        return Ok(());
    }

    // Write back in whichever shape the user already uses, and never report
    // success without having actually inserted: silently skipping here would
    // leave the project untrusted, which fails closed but invisibly.
    match doc.get_mut("projects") {
        Some(projects) if projects.is_inline_table() => {
            let Some(table) = projects.as_inline_table_mut() else {
                unreachable!("just checked is_inline_table")
            };
            let mut entry = toml_edit::InlineTable::new();
            entry.insert("trust_level", "trusted".into());
            table.insert(&key, toml_edit::Value::InlineTable(entry));
        }
        Some(projects) if projects.is_table() => {
            let Some(table) = projects.as_table_mut() else {
                unreachable!("just checked is_table")
            };
            table.set_implicit(true);
            let mut entry = toml_edit::Table::new();
            entry.insert("trust_level", toml_edit::value("trusted"));
            table.insert(&key, toml_edit::Item::Table(entry));
        }
        Some(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "{}: `projects` is not a table; refusing to overwrite it",
                    config_path.display()
                ),
            ));
        }
        None => {
            let mut table = toml_edit::Table::new();
            table.set_implicit(true);
            let mut entry = toml_edit::Table::new();
            entry.insert("trust_level", toml_edit::value("trusted"));
            table.insert(&key, toml_edit::Item::Table(entry));
            doc.insert("projects", toml_edit::Item::Table(table));
        }
    }

    write_atomically(&config_path, &doc.to_string())
}

pub(crate) const CONFIG_LOCK: &str = ".config.toml.lock";
pub(crate) const HOOKS_LOCK: &str = ".hooks.json.lock";

/// Inter-process mutex over one file in the Forall home, so a read and its
/// matching write cannot interleave with another process's pair.
///
/// This is an advisory lock on an open handle (`File::try_lock`, the same
/// primitive `message-history` uses), not a lock *file* whose existence means
/// ownership. That distinction matters: the kernel drops the lock when the
/// handle closes, including when the process is killed, so there is no
/// abandoned-lock state to detect and no takeover to get wrong. An earlier
/// hand-rolled version stamped a token into the file and removed it on drop,
/// which still raced — a successor could take the file over between the
/// departing holder's read of the token and its unlink, and then have its own
/// lock deleted out from under it.
///
/// Acquisition is still best-effort: if it cannot be taken within the retry
/// budget we proceed anyway, because the atomic write behind it already rules
/// out the destructive interleavings on its own.
pub(crate) struct HomeFileLock {
    /// Held only for its lock; dropping the handle releases it.
    _file: fs::File,
}

impl HomeFileLock {
    const MAX_RETRIES: usize = 500;
    const RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(20);

    /// `name` is the lock file, one per protected file: separate files do not
    /// contend with each other, and bootstrap writes several in a row.
    pub(crate) fn acquire(home: &Path, name: &str) -> Option<Self> {
        Self::acquire_within(home, name, Self::MAX_RETRIES)
    }

    fn acquire_within(home: &Path, name: &str, retries: usize) -> Option<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(home.join(name))
            .ok()?;
        for _ in 0..retries {
            match file.try_lock() {
                Ok(()) => return Some(Self { _file: file }),
                Err(fs::TryLockError::WouldBlock) => std::thread::sleep(Self::RETRY_SLEEP),
                Err(_) => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod trust_tests {
    use super::*;

    fn projects(home: &Path) -> toml_edit::DocumentMut {
        fs::read_to_string(home.join("config.toml"))
            .expect("config")
            .parse()
            .expect("config is valid TOML")
    }

    /// The regression that destroyed a real config: adding one trust entry
    /// rewrites the whole file, and the old code read it and re-wrote it with
    /// a truncating `fs::write`. Concurrent writers read each other's
    /// half-written files and persisted the halves — 1200 entries collapsed
    /// to three in one observed round, and to unparseable TOML in another.
    #[test]
    fn concurrent_trust_writes_keep_every_existing_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let mut seed = String::from("model_provider = \"anthropic\"\n\n");
        for index in 0..200 {
            seed.push_str(&format!(
                "[projects.\"/existing/project-{index:03}\"]\ntrust_level = \"trusted\"\n"
            ));
        }
        fs::write(home.join("config.toml"), &seed).expect("seed config");

        std::thread::scope(|scope| {
            for index in 0..16 {
                scope.spawn(move || {
                    let project = PathBuf::from(format!("/new/project-{index:03}"));
                    trust_project_in_home(home, &project).expect("trust");
                });
            }
        });

        let doc = projects(home);
        let table = doc["projects"].as_table().expect("projects table");
        assert_eq!(table.len(), 216, "no pre-existing or new entry may be lost");
        assert_eq!(
            doc["model_provider"].as_str(),
            Some("anthropic"),
            "unrelated settings survive"
        );
    }

    /// The old already-trusted check fell back to a bare substring scan of the
    /// whole file, so /a/b was considered trusted whenever /a/bc was.
    #[test]
    fn a_path_that_prefixes_a_trusted_one_is_still_trusted_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        trust_project_in_home(home, Path::new("/work/demo-2")).expect("trust longer");
        trust_project_in_home(home, Path::new("/work/demo")).expect("trust prefix");

        let doc = projects(home);
        let table = doc["projects"].as_table().expect("projects table");
        assert!(table.contains_key("/work/demo"), "prefix path was skipped");
        assert!(table.contains_key("/work/demo-2"));
    }

    /// A config that spells `projects` as an inline table is valid TOML, and
    /// the first cut silently no-opped on it: the mutable accessor for a
    /// standard table returns None, so nothing was inserted, the unchanged
    /// document was written back, and Ok was returned. The project stayed
    /// untrusted with no error to show for it.
    #[test]
    fn an_inline_projects_table_is_extended_not_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        fs::write(
            home.join("config.toml"),
            "model_provider = \"anthropic\"\nprojects = { \"/already/trusted\" = { trust_level = \"trusted\" } }\n",
        )
        .expect("seed");

        trust_project_in_home(home, Path::new("/work/demo")).expect("trust");

        let doc = projects(home);
        let table = doc["projects"].as_table_like().expect("projects");
        assert!(table.get("/work/demo").is_some(), "new project was dropped");
        assert!(table.get("/already/trusted").is_some(), "existing lost");
    }

    /// The already-trusted check must see inline entries too, or trusting an
    /// inline-listed project would append a duplicate key.
    #[test]
    fn an_inline_entry_counts_as_already_trusted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let seed = "projects = { \"/work/demo\" = { trust_level = \"trusted\" } }\n";
        fs::write(home.join("config.toml"), seed).expect("seed");

        trust_project_in_home(home, Path::new("/work/demo")).expect("trust");

        assert_eq!(
            fs::read_to_string(home.join("config.toml")).expect("read"),
            seed,
            "an already-trusted project rewrites nothing"
        );
    }

    /// `projects` holding something that is not a table at all must be an
    /// error, never a silent success that reports the project as trusted.
    #[test]
    fn a_projects_key_that_is_not_a_table_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let seed = "projects = \"nonsense\"\n";
        fs::write(home.join("config.toml"), seed).expect("seed");

        let err = trust_project_in_home(home, Path::new("/work/demo")).expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).expect("read"),
            seed
        );
    }

    /// Trusting twice must not duplicate the table (toml would reject it).
    #[test]
    fn trusting_the_same_project_twice_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        trust_project_in_home(home, Path::new("/work/demo")).expect("first");
        trust_project_in_home(home, Path::new("/work/demo")).expect("second");

        let doc = projects(home);
        assert_eq!(doc["projects"].as_table().expect("table").len(), 1);
    }

    /// The lock must actually exclude: a second holder cannot take it while
    /// the first is alive. A short retry budget keeps the test fast.
    #[test]
    fn a_held_lock_excludes_a_second_holder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();

        let held = HomeFileLock::acquire(home, CONFIG_LOCK).expect("first holder");
        assert!(
            HomeFileLock::acquire_within(home, CONFIG_LOCK, 2).is_none(),
            "a second holder took a lock that was already held"
        );

        drop(held);
        assert!(
            HomeFileLock::acquire(home, CONFIG_LOCK).is_some(),
            "the lock was not released when its holder dropped"
        );
    }

    /// Locks are per protected file, so bootstrap writing config.toml and
    /// hooks.json back to back does not serialize against itself.
    #[test]
    fn different_files_do_not_contend() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();

        let _config = HomeFileLock::acquire(home, CONFIG_LOCK).expect("config lock");
        assert!(
            HomeFileLock::acquire_within(home, HOOKS_LOCK, 2).is_some(),
            "an unrelated file's lock was blocked"
        );
    }

    /// A config we cannot parse is a config we must not overwrite — the old
    /// code appended to it blindly, compounding the damage.
    #[test]
    fn a_corrupt_config_is_reported_not_clobbered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path();
        let broken = "[projects.\"/half/written\n";
        fs::write(home.join("config.toml"), broken).expect("seed");

        let err = trust_project_in_home(home, Path::new("/work/demo")).expect_err("must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).expect("still there"),
            broken,
            "the damaged file is left exactly as found"
        );
    }
}
