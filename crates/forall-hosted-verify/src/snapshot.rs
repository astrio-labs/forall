use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::InlineFile;
use crate::VerificationSource;

pub const MAX_SNAPSHOT_FILES: usize = 256;
pub const MAX_FILE_BYTES: u64 = 512 * 1024;
pub const MAX_SNAPSHOT_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct SnapshotPacker {
    max_files: usize,
    max_file_bytes: u64,
    max_total_bytes: u64,
}

impl Default for SnapshotPacker {
    fn default() -> Self {
        Self {
            max_files: MAX_SNAPSHOT_FILES,
            max_file_bytes: MAX_FILE_BYTES,
            max_total_bytes: MAX_SNAPSHOT_BYTES,
        }
    }
}

impl SnapshotPacker {
    /// Pack the verification inputs for a Forall workspace without uploading the
    /// entire repository. This includes `.forall` project truth, mapped code and
    /// property files, and root build manifests needed by the remote runner.
    pub fn pack_verification_workspace(
        &self,
        root: &Path,
    ) -> Result<VerificationSource, SnapshotError> {
        let root = canonical_root(root)?;
        let mut paths = BTreeSet::new();
        collect_forall_files(&root, &mut paths)?;
        collect_mapped_files(&root, &mut paths)?;
        collect_support_files(&root, &mut paths)?;
        self.pack_canonical_paths(&root, paths)
    }

    pub fn pack_workspace(&self, root: &Path) -> Result<VerificationSource, SnapshotError> {
        let root = canonical_root(root)?;
        let entries = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| should_descend(&root, entry));
        let mut paths = BTreeSet::new();
        for entry in entries {
            let entry = entry.map_err(|error| SnapshotError::Walk(error.to_string()))?;
            if entry.path() == root {
                continue;
            }
            if entry.file_type().is_symlink() {
                return Err(SnapshotError::Symlink(relative_path(&root, entry.path())?));
            }
            if entry.file_type().is_file() && !is_secret_file(entry.path()) {
                paths.insert(entry.path().to_path_buf());
            }
        }
        self.pack_canonical_paths(&root, paths)
    }

    pub fn pack_paths<I, P>(
        &self,
        root: &Path,
        paths: I,
    ) -> Result<VerificationSource, SnapshotError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let root = canonical_root(root)?;
        let mut selected = BTreeSet::new();
        for path in paths {
            let relative = validate_relative(path.as_ref())?;
            let candidate = root.join(relative);
            reject_symlink_components(&root, &candidate)?;
            let canonical = candidate
                .canonicalize()
                .map_err(|source| SnapshotError::Io {
                    path: candidate.clone(),
                    source,
                })?;
            if !canonical.starts_with(&root) {
                return Err(SnapshotError::OutsideRoot(candidate));
            }
            let metadata = fs::metadata(&canonical).map_err(|source| SnapshotError::Io {
                path: canonical.clone(),
                source,
            })?;
            if !metadata.is_file() {
                return Err(SnapshotError::NotRegularFile(canonical));
            }
            if is_secret_file(&canonical) || is_excluded_component(&root, &canonical) {
                return Err(SnapshotError::Excluded(relative_path(&root, &canonical)?));
            }
            selected.insert(canonical);
        }
        self.pack_canonical_paths(&root, selected)
    }

    fn pack_canonical_paths(
        &self,
        root: &Path,
        paths: BTreeSet<PathBuf>,
    ) -> Result<VerificationSource, SnapshotError> {
        if paths.is_empty() {
            return Err(SnapshotError::EmptySnapshot);
        }
        if paths.len() > self.max_files {
            return Err(SnapshotError::TooManyFiles {
                count: paths.len(),
                limit: self.max_files,
            });
        }
        let mut total_bytes = 0_u64;
        let mut files = Vec::with_capacity(paths.len());
        for path in paths {
            let metadata = fs::metadata(&path).map_err(|source| SnapshotError::Io {
                path: path.clone(),
                source,
            })?;
            if !metadata.is_file() {
                return Err(SnapshotError::NotRegularFile(path));
            }
            if metadata.len() > self.max_file_bytes {
                return Err(SnapshotError::FileTooLarge {
                    path: relative_path(root, &path)?,
                    size: metadata.len(),
                    limit: self.max_file_bytes,
                });
            }
            total_bytes = total_bytes.saturating_add(metadata.len());
            if total_bytes > self.max_total_bytes {
                return Err(SnapshotError::SnapshotTooLarge {
                    size: total_bytes,
                    limit: self.max_total_bytes,
                });
            }
            let bytes = fs::read(&path).map_err(|source| SnapshotError::Io {
                path: path.clone(),
                source,
            })?;
            let relative = relative_path(root, &path)?;
            let content = String::from_utf8(bytes).map_err(|_| SnapshotError::NonUtf8(relative))?;
            files.push(InlineFile {
                path: portable_relative_path(root, &path)?,
                content,
            });
        }
        Ok(VerificationSource::Inline { files })
    }
}

fn collect_forall_files(root: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<(), SnapshotError> {
    let forall_dir = root.join(".forall");
    // `WalkDir` follows a symlinked walk root even with `follow_links(false)`,
    // so the root has to be rejected here rather than per entry below.
    let Some(metadata) = optional_symlink_metadata(&forall_dir)? else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        return Err(SnapshotError::Symlink(relative_path(root, &forall_dir)?));
    }
    if !metadata.is_dir() {
        return Ok(());
    }
    let entries = WalkDir::new(&forall_dir).follow_links(false).into_iter();
    for entry in entries {
        let entry = entry.map_err(|error| SnapshotError::Walk(error.to_string()))?;
        if entry.path() == forall_dir {
            continue;
        }
        let relative = relative_path(root, entry.path())?;
        if relative.starts_with(".forall/verify/cache") {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(SnapshotError::Symlink(relative));
        }
        if entry.file_type().is_file() && !is_secret_file(entry.path()) {
            paths.insert(entry.path().to_path_buf());
        }
    }
    Ok(())
}

fn collect_mapped_files(root: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<(), SnapshotError> {
    let mut mappings = Vec::new();
    let project_mapping = root.join(".forall/verify/mapping.yaml");
    if project_mapping.is_file() {
        mappings.push(project_mapping);
    }
    let changes = root.join(".forall/workflow/changes");
    if changes.is_dir() {
        for entry in WalkDir::new(&changes).max_depth(3).follow_links(false) {
            let entry = entry.map_err(|error| SnapshotError::Walk(error.to_string()))?;
            if entry.file_type().is_file()
                && entry.file_name().to_string_lossy() == "mapping.delta.yaml"
            {
                mappings.push(entry.path().to_path_buf());
            }
        }
    }

    for mapping_path in mappings {
        let content = fs::read_to_string(&mapping_path).map_err(|source| SnapshotError::Io {
            path: mapping_path.clone(),
            source,
        })?;
        let mapping: serde_yaml::Value =
            serde_yaml::from_str(&content).map_err(|source| SnapshotError::InvalidMapping {
                path: relative_path(root, &mapping_path).unwrap_or(mapping_path.clone()),
                detail: source.to_string(),
            })?;
        collect_mapping_file_fields(root, &mapping, paths)?;
    }
    Ok(())
}

fn collect_mapping_file_fields(
    root: &Path,
    value: &serde_yaml::Value,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), SnapshotError> {
    match value {
        serde_yaml::Value::Mapping(mapping) => {
            for (key, value) in mapping {
                if key.as_str() == Some("file")
                    && let Some(path) = value.as_str()
                {
                    add_selected_file(root, Path::new(path), paths)?;
                }
                collect_mapping_file_fields(root, value, paths)?;
            }
        }
        serde_yaml::Value::Sequence(values) => {
            for value in values {
                collect_mapping_file_fields(root, value, paths)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn collect_support_files(root: &Path, paths: &mut BTreeSet<PathBuf>) -> Result<(), SnapshotError> {
    const EXACT: &[&str] = &[
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lock",
        "bun.lockb",
        "tsconfig.json",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain",
        "rust-toolchain.toml",
        "pom.xml",
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
        "gradle.properties",
    ];
    for relative in EXACT {
        let path = root.join(relative);
        // `Path::is_file` follows symlinks, which would pull a file from
        // outside the workspace into the snapshot.
        let Some(metadata) = optional_symlink_metadata(&path)? else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(SnapshotError::Symlink(relative_path(root, &path)?));
        }
        if metadata.is_file() {
            paths.insert(path);
        }
    }
    Ok(())
}

/// Metadata for `path` without following a terminal symlink, treating a
/// missing path as `None` rather than an error.
fn optional_symlink_metadata(path: &Path) -> Result<Option<fs::Metadata>, SnapshotError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SnapshotError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn add_selected_file(
    root: &Path,
    relative: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), SnapshotError> {
    let relative = validate_relative(relative)?;
    let candidate = root.join(relative);
    reject_symlink_components(root, &candidate)?;
    let canonical = candidate
        .canonicalize()
        .map_err(|source| SnapshotError::Io {
            path: candidate.clone(),
            source,
        })?;
    if !canonical.starts_with(root) {
        return Err(SnapshotError::OutsideRoot(candidate));
    }
    if !canonical.is_file() {
        return Err(SnapshotError::NotRegularFile(canonical));
    }
    if is_secret_file(&canonical) || is_excluded_component(root, &canonical) {
        return Err(SnapshotError::Excluded(relative_path(root, &canonical)?));
    }
    paths.insert(canonical);
    Ok(())
}

pub fn github_source(
    repository: impl Into<String>,
    reference: impl Into<String>,
    subdirectory: Option<String>,
) -> VerificationSource {
    VerificationSource::github(repository, reference, subdirectory)
}

fn canonical_root(root: &Path) -> Result<PathBuf, SnapshotError> {
    let root = root.canonicalize().map_err(|source| SnapshotError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !root.is_dir() {
        return Err(SnapshotError::RootNotDirectory(root));
    }
    Ok(root)
}

fn validate_relative(path: &Path) -> Result<&Path, SnapshotError> {
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(SnapshotError::InvalidPath(path.to_path_buf()));
    }
    Ok(path)
}

fn reject_symlink_components(root: &Path, candidate: &Path) -> Result<(), SnapshotError> {
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| SnapshotError::OutsideRoot(candidate.to_path_buf()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|source| SnapshotError::Io {
            path: current.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(SnapshotError::Symlink(relative_path(root, &current)?));
        }
    }
    Ok(())
}

fn should_descend(root: &Path, entry: &DirEntry) -> bool {
    if entry.path() == root {
        return true;
    }
    !is_excluded_component(root, entry.path()) && !is_secret_file(entry.path())
}

fn is_excluded_component(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative.components().any(|component| {
            let name = component.as_os_str().to_string_lossy();
            matches!(
                name.as_ref(),
                ".git"
                    | ".hg"
                    | ".svn"
                    | "target"
                    | "node_modules"
                    | "dist"
                    | "build"
                    | "out"
                    | ".next"
                    | ".turbo"
                    | "coverage"
                    | "__pycache__"
            )
        })
    })
}

fn is_secret_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower == "credentials.json"
        || lower == "secrets.json"
        || lower == "id_rsa"
        || lower == "id_ed25519"
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
}

fn relative_path(root: &Path, path: &Path) -> Result<PathBuf, SnapshotError> {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .map_err(|_| SnapshotError::OutsideRoot(path.to_path_buf()))
}

fn portable_relative_path(root: &Path, path: &Path) -> Result<String, SnapshotError> {
    let relative = relative_path(root, path)?;
    let parts = relative
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| SnapshotError::NonUtf8Path(relative.clone()))?;
    Ok(parts.join("/"))
}

#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("verification snapshot contains no files")]
    EmptySnapshot,
    #[error("workspace root is not a directory: {0}")]
    RootNotDirectory(PathBuf),
    #[error("invalid workspace-relative path: {0}")]
    InvalidPath(PathBuf),
    #[error("path escapes workspace root: {0}")]
    OutsideRoot(PathBuf),
    #[error("symlinks are not allowed in snapshots: {0}")]
    Symlink(PathBuf),
    #[error("path is not a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("path is excluded from snapshots: {0}")]
    Excluded(PathBuf),
    #[error("file path is not UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("file content is not UTF-8: {0}")]
    NonUtf8(PathBuf),
    #[error("snapshot has {count} files; limit is {limit}")]
    TooManyFiles { count: usize, limit: usize },
    #[error("file {path} is {size} bytes; limit is {limit}")]
    FileTooLarge {
        path: PathBuf,
        size: u64,
        limit: u64,
    },
    #[error("snapshot is {size} bytes; limit is {limit}")]
    SnapshotTooLarge { size: u64, limit: u64 },
    #[error("failed to walk workspace: {0}")]
    Walk(String),
    #[error("mapping file {path} is invalid: {detail}")]
    InvalidMapping { path: PathBuf, detail: String },
    #[error("filesystem operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
