use dirs::home_dir;
use forall_utils_absolute_path::AbsolutePathBuf;
use std::path::PathBuf;

/// Returns the path to the Forall configuration directory.
///
/// Resolution order:
/// 1. `FORALL_HOME` when set
/// 2. deprecated `CODEX_HOME` when set
/// 3. `~/.forall` when it exists
/// 4. `~/.codex` when it exists (legacy installs)
/// 5. `~/.forall` as the default for new installs
pub fn find_forall_home() -> std::io::Result<AbsolutePathBuf> {
    let forall_home_env = std::env::var("FORALL_HOME")
        .ok()
        .or_else(|| std::env::var("CODEX_HOME").ok())
        .filter(|val| !val.is_empty());
    find_forall_home_from_env(forall_home_env.as_deref())
}

fn default_home_dir() -> PathBuf {
    let mut home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    let forall_home = home.join(".forall");
    let legacy_home = {
        let mut legacy = home.clone();
        legacy.push(".codex");
        legacy
    };

    if forall_home.is_dir() {
        return forall_home;
    }
    if legacy_home.is_dir() {
        return legacy_home;
    }
    home.push(".forall");
    forall_home
}

fn resolve_home_from_env(val: &str) -> std::io::Result<AbsolutePathBuf> {
    let path = PathBuf::from(val);
    let metadata = std::fs::metadata(&path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("FORALL_HOME points to {val:?}, but that path does not exist"),
        ),
        _ => std::io::Error::new(
            err.kind(),
            format!("failed to read FORALL_HOME {val:?}: {err}"),
        ),
    })?;

    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("FORALL_HOME points to {val:?}, but that path is not a directory"),
        ));
    }

    let canonical = path.canonicalize().map_err(|err| {
        std::io::Error::new(
            err.kind(),
            format!("failed to canonicalize FORALL_HOME {val:?}: {err}"),
        )
    })?;
    AbsolutePathBuf::from_absolute_path(canonical)
}

fn find_forall_home_from_env(forall_home_env: Option<&str>) -> std::io::Result<AbsolutePathBuf> {
    match forall_home_env {
        Some(val) => resolve_home_from_env(val),
        None => {
            let default = default_home_dir();
            AbsolutePathBuf::from_absolute_path(default)
        }
    }
}

#[deprecated(note = "use find_forall_home")]
pub fn find_codex_home() -> std::io::Result<AbsolutePathBuf> {
    find_forall_home()
}

#[cfg(test)]
mod tests {
    use super::find_forall_home_from_env;
    use super::resolve_home_from_env;
    use forall_utils_absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::io::ErrorKind;
    use tempfile::TempDir;

    #[test]
    fn find_forall_home_env_missing_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let missing = temp_home.path().join("missing-forall-home");
        let missing_str = missing
            .to_str()
            .expect("missing forall home path should be valid utf-8");

        let err = find_forall_home_from_env(Some(missing_str)).expect_err("missing FORALL_HOME");
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(
            err.to_string().contains("FORALL_HOME"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_forall_home_env_file_path_is_fatal() {
        let temp_home = TempDir::new().expect("temp home");
        let file_path = temp_home.path().join("forall-home.txt");
        fs::write(&file_path, "not a directory").expect("write temp file");
        let file_str = file_path
            .to_str()
            .expect("file forall home path should be valid utf-8");

        let err = find_forall_home_from_env(Some(file_str)).expect_err("file FORALL_HOME");
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
        assert!(
            err.to_string().contains("not a directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn find_forall_home_env_valid_directory_canonicalizes() {
        let temp_home = TempDir::new().expect("temp home");
        let temp_str = temp_home
            .path()
            .to_str()
            .expect("temp forall home path should be valid utf-8");

        let resolved = resolve_home_from_env(temp_str).expect("valid FORALL_HOME");
        let expected = temp_home
            .path()
            .canonicalize()
            .expect("canonicalize temp home");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn find_forall_home_without_env_uses_default_home_dir() {
        let resolved =
            find_forall_home_from_env(/*forall_home_env*/ None).expect("default FORALL_HOME");
        let mut expected = dirs::home_dir().expect("home dir");
        expected.push(".forall");
        let expected = AbsolutePathBuf::from_absolute_path(expected).expect("absolute home");
        assert_eq!(resolved, expected);
    }
}
