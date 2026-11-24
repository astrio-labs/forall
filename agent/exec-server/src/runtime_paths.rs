use std::path::PathBuf;

use forall_utils_absolute_path::AbsolutePathBuf;

/// Runtime paths needed by exec-server child processes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecServerRuntimePaths {
    /// Stable path to the Forall executable used to launch hidden helper modes.
    pub forall_self_exe: AbsolutePathBuf,
    /// Path to the Linux sandbox helper alias used when the platform sandbox
    /// needs to re-enter Forall by argv0.
    pub forall_linux_sandbox_exe: Option<AbsolutePathBuf>,
}

impl ExecServerRuntimePaths {
    pub fn from_optional_paths(
        forall_self_exe: Option<PathBuf>,
        forall_linux_sandbox_exe: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        let forall_self_exe = forall_self_exe.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Forall executable path is not configured",
            )
        })?;
        Self::new(forall_self_exe, forall_linux_sandbox_exe)
    }

    pub fn new(
        forall_self_exe: PathBuf,
        forall_linux_sandbox_exe: Option<PathBuf>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            forall_self_exe: absolute_path(forall_self_exe)?,
            forall_linux_sandbox_exe: forall_linux_sandbox_exe.map(absolute_path).transpose()?,
        })
    }
}

fn absolute_path(path: PathBuf) -> std::io::Result<AbsolutePathBuf> {
    AbsolutePathBuf::from_absolute_path(path.as_path())
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))
}
