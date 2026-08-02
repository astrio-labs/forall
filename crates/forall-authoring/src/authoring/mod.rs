use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mapping::schema::{Mapping, PropertyRef, Requirement, validate_mapping};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAPPING_PATH: &str = ".forall/verify/mapping.yaml";

// The parameter group in each pattern tolerates one level of nesting
// (`(?:[^()]|\([^()]*\))*`) so a parameter such as `cb: fn(u32) -> u32` does
// not truncate the captured signature at its inner `)`. The optional generic
// group before it lists `->`/`=>` first so an arrow inside a bound is consumed
// whole rather than read as the closing `>`.
static TYPESCRIPT_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*export[ \t]+(?:default[ \t]+)?(?:async[ \t]+)?function[ \t]+([A-Za-z_$][\w$]*)(?:<(?:=>|[^<>]|<[^<>]*>)*>)?[ \t]*(\((?:[^()]|\([^()]*\))*\))",
    )
    .expect("TypeScript function pattern is valid")
});

static TYPESCRIPT_ARROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*export[ \t]+(?:const|let)[ \t]+([A-Za-z_$][\w$]*)[ \t]*(?::[^=\n]+)?=[ \t]*(?:async[ \t]+)?(\((?:[^()]|\([^()]*\))*\))[ \t]*(?::[^=\n]+)?=>[ \t]*\{",
    )
    .expect("TypeScript arrow pattern is valid")
});

static RUST_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*pub(?:\([^)]*\))?[ \t]+(?:(?:async|const|unsafe)[ \t]+)*fn[ \t]+([A-Za-z_]\w*)(?:<(?:->|[^<>]|<[^<>]*>)*>)?[ \t]*(\((?:[^()]|\([^()]*\))*\))",
    )
    .expect("Rust function pattern is valid")
});

static JAVA_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^[ \t]*public[ \t]+(?:static[ \t]+)?(?:final[ \t]+)?[\w<>\[\], ?]+[ \t]+([A-Za-z_]\w*)[ \t]*(\((?:[^()]|\([^()]*\))*\))[ \t]*(?:throws[^{]+)?\{",
    )
    .expect("Java method pattern is valid")
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoringErrorCode {
    InvalidRoot,
    UnsafePath,
    NotFound,
    Conflict,
    Malformed,
    StaleContent,
    AmbiguousSymbol,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringError {
    pub code: AuthoringErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

pub type AuthoringResult<T> = Result<T, AuthoringError>;

fn error(code: AuthoringErrorCode, message: impl Into<String>) -> AuthoringError {
    AuthoringError {
        code,
        message: message.into(),
        path: None,
    }
}

fn path_error(
    code: AuthoringErrorCode,
    path: impl Into<String>,
    message: impl Into<String>,
) -> AuthoringError {
    AuthoringError {
        code,
        message: message.into(),
        path: Some(path.into()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRoot(PathBuf);

impl CanonicalRoot {
    pub fn new(root: impl AsRef<Path>) -> AuthoringResult<Self> {
        let root = root.as_ref().canonicalize().map_err(|err| {
            error(
                AuthoringErrorCode::InvalidRoot,
                format!("project root is unavailable: {err}"),
            )
        })?;
        if !root.is_dir() {
            return Err(error(
                AuthoringErrorCode::InvalidRoot,
                "project root must be a directory",
            ));
        }
        Ok(Self(root))
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    fn resolve(&self, relative: &str) -> AuthoringResult<PathBuf> {
        let rel = Path::new(relative);
        if relative.is_empty() || rel.is_absolute() {
            return Err(path_error(
                AuthoringErrorCode::UnsafePath,
                relative,
                "path must be a non-empty project-relative path",
            ));
        }
        if rel.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(path_error(
                AuthoringErrorCode::UnsafePath,
                relative,
                "path traversal is not allowed",
            ));
        }

        let candidate = self.0.join(rel);
        let mut existing = candidate.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or_else(|| {
                path_error(
                    AuthoringErrorCode::UnsafePath,
                    relative,
                    "path has no project parent",
                )
            })?;
        }
        let canonical = existing.canonicalize().map_err(|err| {
            path_error(
                AuthoringErrorCode::Io,
                relative,
                format!("cannot resolve path: {err}"),
            )
        })?;
        if !canonical.starts_with(&self.0) {
            return Err(path_error(
                AuthoringErrorCode::UnsafePath,
                relative,
                "path resolves outside the project root",
            ));
        }
        if candidate.exists() {
            let canonical_target = candidate.canonicalize().map_err(|err| {
                path_error(
                    AuthoringErrorCode::Io,
                    relative,
                    format!("cannot resolve path: {err}"),
                )
            })?;
            if !canonical_target.starts_with(&self.0) {
                return Err(path_error(
                    AuthoringErrorCode::UnsafePath,
                    relative,
                    "path resolves outside the project root",
                ));
            }
        }
        Ok(candidate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationMode {
    Preview,
    Apply,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationOptions {
    pub mode: MutationMode,
    #[serde(default)]
    pub expected_sha256: BTreeMap<String, String>,
}

impl Default for MutationOptions {
    fn default() -> Self {
        Self {
            mode: MutationMode::Preview,
            expected_sha256: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationOutput {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub unchanged: Vec<String>,
    pub warnings: Vec<String>,
    pub files: Vec<FileMutation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMutationAction {
    Created,
    Updated,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMutation {
    pub path: String,
    pub action: FileMutationAction,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectStatus {
    pub initialized: bool,
    pub mapping_path: String,
    pub mapping_sha256: Option<String>,
    pub requirement_count: usize,
    pub languages: Vec<SourceLanguage>,
    pub mapped_symbols: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn project_status(root: &CanonicalRoot) -> AuthoringResult<ProjectStatus> {
    let path = root.resolve(MAPPING_PATH)?;
    if !path.exists() {
        return Ok(ProjectStatus {
            initialized: false,
            mapping_path: MAPPING_PATH.to_string(),
            mapping_sha256: None,
            requirement_count: 0,
            languages: detect_languages(root)?,
            mapped_symbols: Vec::new(),
            warnings: Vec::new(),
        });
    }
    let bytes = read_regular_file(root, MAPPING_PATH)?;
    let mapping: Mapping = serde_yaml::from_slice(&bytes).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is malformed: {err}"),
        )
    })?;
    validate_mapping(&mapping).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is invalid: {err}"),
        )
    })?;
    Ok(ProjectStatus {
        initialized: true,
        mapping_path: MAPPING_PATH.to_string(),
        mapping_sha256: Some(sha256(&bytes)),
        requirement_count: mapping.requirements.len(),
        languages: detect_languages(root)?,
        mapped_symbols: mapping
            .requirements
            .iter()
            .filter_map(|requirement| requirement.code.as_ref())
            .flat_map(|code| {
                code.symbols
                    .iter()
                    .map(|symbol| format!("{}::{symbol}", code.file))
            })
            .collect(),
        warnings: Vec::new(),
    })
}

pub fn init_project(
    root: &CanonicalRoot,
    options: &MutationOptions,
) -> AuthoringResult<MutationOutput> {
    let defaults = [
        (MAPPING_PATH, "version: 1\nrequirements: []\n"),
        (
            ".forall/workflow/config.yaml",
            "schema: forall\ncontext: |\n  This project uses Forall contracts and verification.\nrules:\n  verification:\n    - Keep verified requirements machine-checked before claiming success\n",
        ),
        (
            ".forall/AGENTS.md",
            "# Forall\n\nAuthor requirements and contracts locally, validate them, then run hosted Forall verification.\n",
        ),
    ];
    let mut writes = Vec::new();
    let mut output = MutationOutput::default();
    for (relative, content) in defaults {
        let path = root.resolve(relative)?;
        if path.exists() {
            let current = read_regular_file(root, relative)?;
            output.unchanged.push(relative.to_string());
            output.files.push(FileMutation {
                path: relative.to_string(),
                action: FileMutationAction::Unchanged,
                before_sha256: Some(sha256(&current)),
                after_sha256: sha256(&current),
                proposed_content: None,
            });
        } else {
            writes.push((relative.to_string(), content.as_bytes().to_vec()));
        }
    }
    let created = apply_writes(root, writes, options)?;
    output.created.extend(created.created);
    output.updated.extend(created.updated);
    output.files.extend(created.files);
    Ok(output)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceLanguage {
    TypeScript,
    Rust,
    Java,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredSymbol {
    pub file: String,
    pub language: SourceLanguage,
    /// Stable selector accepted by `scaffold_contracts`.
    pub symbol: String,
    pub name: String,
    pub line: usize,
    pub signature: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverSymbolsOutput {
    pub symbols: Vec<DiscoveredSymbol>,
    pub files: Vec<DiscoveredFile>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredFile {
    pub path: String,
    pub sha256: String,
}

pub fn discover_symbols(
    root: &CanonicalRoot,
    files: &[String],
) -> AuthoringResult<DiscoverSymbolsOutput> {
    let mut output = DiscoverSymbolsOutput::default();
    for file in files {
        let bytes = read_regular_file(root, file)?;
        output.files.push(DiscoveredFile {
            path: file.clone(),
            sha256: sha256(&bytes),
        });
        let source = String::from_utf8(bytes).map_err(|_| {
            path_error(
                AuthoringErrorCode::Malformed,
                file,
                "source file is not valid UTF-8",
            )
        })?;
        let language = language_for_path(file).ok_or_else(|| {
            path_error(
                AuthoringErrorCode::Malformed,
                file,
                "supported source extensions are .ts, .tsx, .rs, and .java",
            )
        })?;
        output
            .symbols
            .extend(parse_symbols(file, language, &source)?);
    }
    output
        .symbols
        .sort_by(|a, b| (&a.file, a.line, &a.symbol).cmp(&(&b.file, b.line, &b.symbol)));
    output.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

pub fn discover_project_symbols(
    root: &CanonicalRoot,
    languages: &[SourceLanguage],
) -> AuthoringResult<DiscoverSymbolsOutput> {
    let selected: BTreeSet<SourceLanguage> = if languages.is_empty() {
        [
            SourceLanguage::TypeScript,
            SourceLanguage::Rust,
            SourceLanguage::Java,
        ]
        .into_iter()
        .collect()
    } else {
        languages.iter().copied().collect()
    };
    let files = source_files(root)?
        .into_iter()
        .filter(|file| language_for_path(file).is_some_and(|language| selected.contains(&language)))
        .collect::<Vec<_>>();
    discover_symbols(root, &files)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertRequirementsRequest {
    pub requirements: Vec<Requirement>,
    pub mutation: MutationOptions,
}

pub fn upsert_requirements(
    root: &CanonicalRoot,
    request: &UpsertRequirementsRequest,
) -> AuthoringResult<MutationOutput> {
    let bytes = read_regular_file(root, MAPPING_PATH)?;
    let mut mapping: Mapping = serde_yaml::from_slice(&bytes).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is malformed: {err}"),
        )
    })?;
    validate_mapping_uniqueness(&mapping)?;
    let mut seen = BTreeSet::new();
    for req in &request.requirements {
        if !seen.insert(req.id.clone()) {
            return Err(error(
                AuthoringErrorCode::Conflict,
                format!("duplicate requirement id '{}' in request", req.id),
            ));
        }
        if let Some(code) = &req.code {
            for symbol in &code.symbols {
                if let Some(existing) = mapping.requirements.iter().find(|existing| {
                    existing.id != req.id
                        && existing.code.as_ref().is_some_and(|existing_code| {
                            existing_code.file == code.file
                                && existing_code.symbols.contains(symbol)
                        })
                }) {
                    return Err(error(
                        AuthoringErrorCode::Conflict,
                        format!(
                            "{}::{symbol} is already mapped by requirement '{}'",
                            code.file, existing.id
                        ),
                    ));
                }
            }
        }
        if let Some(current) = mapping
            .requirements
            .iter_mut()
            .find(|current| current.id == req.id)
        {
            *current = req.clone();
        } else {
            mapping.requirements.push(req.clone());
        }
    }
    validate_mapping(&mapping).map_err(|err| {
        error(
            AuthoringErrorCode::Malformed,
            format!("requirements are invalid: {err}"),
        )
    })?;
    let rendered = serde_yaml::to_string(&mapping)
        .map_err(|err| error(AuthoringErrorCode::Malformed, err.to_string()))?;
    apply_writes(
        root,
        vec![(MAPPING_PATH.to_string(), preserve_eol(&bytes, &rendered))],
        &request.mutation,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractTarget {
    pub file: String,
    /// Exact selector returned by `discover_symbols`.
    pub symbol: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub ensures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaffoldContractsRequest {
    pub contracts: Vec<ContractTarget>,
    pub mutation: MutationOptions,
}

pub fn scaffold_contracts(
    root: &CanonicalRoot,
    request: &ScaffoldContractsRequest,
) -> AuthoringResult<MutationOutput> {
    let mut grouped: BTreeMap<String, Vec<&ContractTarget>> = BTreeMap::new();
    let mut targets = BTreeSet::new();
    for contract in &request.contracts {
        if !targets.insert((contract.file.clone(), contract.symbol.clone())) {
            return Err(path_error(
                AuthoringErrorCode::Conflict,
                &contract.file,
                format!("duplicate contract target '{}'", contract.symbol),
            ));
        }
        for clause in contract.requires.iter().chain(&contract.ensures) {
            if clause.trim().is_empty() || clause.contains(['\r', '\n']) {
                return Err(path_error(
                    AuthoringErrorCode::Malformed,
                    &contract.file,
                    "contract clauses must be non-empty single lines",
                ));
            }
        }
        grouped
            .entry(contract.file.clone())
            .or_default()
            .push(contract);
    }
    let mut writes = Vec::new();
    for (file, contracts) in grouped {
        let bytes = read_regular_file(root, &file)?;
        let source = String::from_utf8(bytes.clone()).map_err(|_| {
            path_error(
                AuthoringErrorCode::Malformed,
                &file,
                "source file is not valid UTF-8",
            )
        })?;
        let language = language_for_path(&file).ok_or_else(|| {
            path_error(
                AuthoringErrorCode::Malformed,
                &file,
                "unsupported source language",
            )
        })?;
        let symbols = parse_symbols(&file, language, &source)?;
        let mut insertions = Vec::new();
        for target in contracts {
            let matches: Vec<_> = symbols
                .iter()
                .filter(|symbol| symbol.symbol == target.symbol || symbol.name == target.symbol)
                .collect();
            if matches.is_empty() {
                return Err(path_error(
                    AuthoringErrorCode::NotFound,
                    &file,
                    format!("symbol '{}' was not found", target.symbol),
                ));
            }
            if matches.len() > 1 {
                return Err(path_error(
                    AuthoringErrorCode::AmbiguousSymbol,
                    &file,
                    format!(
                        "symbol '{}' is overloaded; use an exact discovery selector",
                        target.symbol
                    ),
                ));
            }
            let symbol = matches[0];
            let insertion = contract_insertion(&source, language, symbol, target)?;
            if let Some(insertion) = insertion {
                insertions.push(insertion);
            }
        }
        insertions.sort_by_key(|insertion| std::cmp::Reverse(insertion.0));
        let mut edited = source;
        for (offset, text) in insertions {
            edited.insert_str(offset, &text);
        }
        writes.push((file, edited.into_bytes()));
    }
    apply_writes(root, writes, &request.mutation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringIssue {
    pub severity: ValidationSeverity,
    pub message: String,
    pub path: Option<String>,
    pub requirement_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidateAuthoringOutput {
    pub valid: bool,
    pub issues: Vec<AuthoringIssue>,
}

pub fn validate_authoring(root: &CanonicalRoot) -> AuthoringResult<ValidateAuthoringOutput> {
    let bytes = read_regular_file(root, MAPPING_PATH)?;
    let mapping: Mapping = serde_yaml::from_slice(&bytes).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is malformed: {err}"),
        )
    })?;
    validate_mapping(&mapping).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is invalid: {err}"),
        )
    })?;
    let mut issues = Vec::new();
    if let Err(err) = validate_mapping_uniqueness(&mapping) {
        issues.push(AuthoringIssue {
            severity: ValidationSeverity::Error,
            message: err.message,
            path: Some(MAPPING_PATH.to_string()),
            requirement_id: None,
        });
    }
    let mut cache: HashMap<String, Vec<DiscoveredSymbol>> = HashMap::new();
    for req in &mapping.requirements {
        if let Some(code) = &req.code {
            let path = match root.resolve(&code.file) {
                Ok(path) => path,
                Err(err) => {
                    issues.push(issue(req, err.message, Some(code.file.clone())));
                    continue;
                }
            };
            if !path.is_file() {
                issues.push(issue(
                    req,
                    "referenced code file is missing",
                    Some(code.file.clone()),
                ));
                continue;
            }
            if !cache.contains_key(&code.file) {
                let discovered = discover_symbols(root, std::slice::from_ref(&code.file))?.symbols;
                cache.insert(code.file.clone(), discovered);
            }
            let names = &cache[&code.file];
            for symbol in &code.symbols {
                if !names
                    .iter()
                    .any(|found| found.name == *symbol || found.symbol == *symbol)
                {
                    issues.push(issue(
                        req,
                        format!("referenced symbol '{symbol}' is missing"),
                        Some(code.file.clone()),
                    ));
                }
            }
        }
        if let Some(property) = &req.property {
            match root.resolve(&property.file) {
                Ok(path) if path.is_file() => {}
                Ok(_) => issues.push(issue(
                    req,
                    "referenced property file is missing",
                    Some(property.file.clone()),
                )),
                Err(err) => issues.push(issue(req, err.message, Some(property.file.clone()))),
            }
        }
    }
    Ok(ValidateAuthoringOutput {
        valid: issues.is_empty(),
        issues,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaffoldPropertyRequest {
    pub requirement_id: String,
    /// Complete caller-authored TypeScript property module body.
    pub body: String,
    #[serde(default)]
    pub symbol: Option<String>,
    pub mutation: MutationOptions,
}

pub fn scaffold_property(
    root: &CanonicalRoot,
    request: &ScaffoldPropertyRequest,
) -> AuthoringResult<MutationOutput> {
    if request.body.trim().is_empty() {
        return Err(error(
            AuthoringErrorCode::Malformed,
            "property body must be supplied by the caller",
        ));
    }
    let bytes = read_regular_file(root, MAPPING_PATH)?;
    let mut mapping: Mapping = serde_yaml::from_slice(&bytes).map_err(|err| {
        path_error(
            AuthoringErrorCode::Malformed,
            MAPPING_PATH,
            format!("mapping is malformed: {err}"),
        )
    })?;
    let requirement = mapping
        .requirements
        .iter_mut()
        .find(|req| req.id == request.requirement_id)
        .ok_or_else(|| {
            error(
                AuthoringErrorCode::NotFound,
                format!("requirement '{}' was not found", request.requirement_id),
            )
        })?;
    let property_path = format!(".forall/scenarios/{}.property.ts", request.requirement_id);
    if let Some(existing) = &requirement.property
        && (existing.file != property_path || existing.symbol != request.symbol)
    {
        return Err(error(
            AuthoringErrorCode::Conflict,
            format!(
                "requirement '{}' already links a different property",
                request.requirement_id
            ),
        ));
    }
    if requirement.verified {
        return Err(error(
            AuthoringErrorCode::Conflict,
            "a proved requirement cannot also be property-tested",
        ));
    }
    requirement.property_tested = true;
    requirement.property = Some(PropertyRef {
        file: property_path.clone(),
        symbol: request.symbol.clone(),
    });
    validate_mapping(&mapping).map_err(|err| error(AuthoringErrorCode::Malformed, err))?;
    let rendered = serde_yaml::to_string(&mapping)
        .map_err(|err| error(AuthoringErrorCode::Malformed, err.to_string()))?;
    apply_writes(
        root,
        vec![
            (MAPPING_PATH.to_string(), preserve_eol(&bytes, &rendered)),
            (property_path, request.body.as_bytes().to_vec()),
        ],
        &request.mutation,
    )
}

fn issue(req: &Requirement, message: impl Into<String>, path: Option<String>) -> AuthoringIssue {
    AuthoringIssue {
        severity: ValidationSeverity::Error,
        message: message.into(),
        path,
        requirement_id: Some(req.id.clone()),
    }
}

fn validate_mapping_uniqueness(mapping: &Mapping) -> AuthoringResult<()> {
    let mut ids = BTreeSet::new();
    let mut symbols = BTreeMap::<(String, String), String>::new();
    for requirement in &mapping.requirements {
        if !ids.insert(requirement.id.clone()) {
            return Err(error(
                AuthoringErrorCode::Conflict,
                format!("duplicate requirement id '{}'", requirement.id),
            ));
        }
        if let Some(code) = &requirement.code {
            for symbol in &code.symbols {
                let key = (code.file.clone(), symbol.clone());
                if let Some(existing) = symbols.insert(key.clone(), requirement.id.clone()) {
                    return Err(error(
                        AuthoringErrorCode::Conflict,
                        format!(
                            "{}::{} is mapped by both '{}' and '{}'",
                            key.0, key.1, existing, requirement.id
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn detect_languages(root: &CanonicalRoot) -> AuthoringResult<Vec<SourceLanguage>> {
    let mut languages = BTreeSet::new();
    for file in source_files(root)? {
        if let Some(language) = language_for_path(&file) {
            languages.insert(language);
        }
    }
    Ok(languages.into_iter().collect())
}

fn source_files(root: &CanonicalRoot) -> AuthoringResult<Vec<String>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeSet<String>) -> AuthoringResult<()> {
        if !directory.is_dir() {
            return Ok(());
        }
        let entries = fs::read_dir(directory).map_err(|err| {
            path_error(
                AuthoringErrorCode::Io,
                directory
                    .strip_prefix(root)
                    .unwrap_or(directory)
                    .display()
                    .to_string(),
                format!("cannot scan source directory: {err}"),
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| error(AuthoringErrorCode::Io, err.to_string()))?;
            let file_type = entry
                .file_type()
                .map_err(|err| error(AuthoringErrorCode::Io, err.to_string()))?;
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".forall" | "node_modules" | "target" | "dist" | "build")
                ) {
                    continue;
                }
                visit(root, &path, files)?;
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
                    .replace('\\', "/");
                if language_for_path(&relative).is_some()
                    && !relative.ends_with(".d.ts")
                    && !relative.contains(".test.")
                    && !relative.contains(".spec.")
                {
                    files.insert(relative);
                }
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    for directory in ["src", "lib", "app"] {
        visit(root.as_path(), &root.as_path().join(directory), &mut files)?;
    }
    Ok(files.into_iter().collect())
}

fn language_for_path(file: &str) -> Option<SourceLanguage> {
    match Path::new(file).extension()?.to_str()? {
        "ts" | "tsx" => Some(SourceLanguage::TypeScript),
        "rs" => Some(SourceLanguage::Rust),
        "java" => Some(SourceLanguage::Java),
        _ => None,
    }
}

fn parse_symbols(
    file: &str,
    language: SourceLanguage,
    source: &str,
) -> AuthoringResult<Vec<DiscoveredSymbol>> {
    let regex = match language {
        SourceLanguage::TypeScript => &*TYPESCRIPT_FUNCTION,
        SourceLanguage::Rust => &*RUST_FUNCTION,
        SourceLanguage::Java => &*JAVA_METHOD,
    };
    let mut output = Vec::new();
    for captures in regex.captures_iter(source) {
        let (Some(full), Some(name), Some(params)) =
            (captures.get(0), captures.get(1), captures.get(2))
        else {
            continue;
        };
        let name = name.as_str().to_string();
        let params = params.as_str();
        let signature = format!("{name}{params}");
        let selector = if language == SourceLanguage::Java {
            signature.clone()
        } else {
            name.clone()
        };
        output.push(DiscoveredSymbol {
            file: file.to_string(),
            language,
            symbol: selector,
            name,
            line: source[..full.start()]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            signature,
        });
    }
    if language == SourceLanguage::TypeScript {
        for captures in TYPESCRIPT_ARROW.captures_iter(source) {
            let (Some(full), Some(name), Some(params)) =
                (captures.get(0), captures.get(1), captures.get(2))
            else {
                continue;
            };
            let name = name.as_str().to_string();
            let params = params.as_str();
            output.push(DiscoveredSymbol {
                file: file.to_string(),
                language,
                symbol: name.clone(),
                name: name.clone(),
                line: source[..full.start()]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1,
                signature: format!("{name}{params}"),
            });
        }
    }
    output.sort_by_key(|symbol| symbol.line);
    Ok(output)
}

fn contract_insertion(
    source: &str,
    language: SourceLanguage,
    symbol: &DiscoveredSymbol,
    target: &ContractTarget,
) -> AuthoringResult<Option<(usize, String)>> {
    if target.requires.is_empty() && target.ensures.is_empty() {
        return Ok(None);
    }
    let line_start = line_start_offset(source, symbol.line);
    let indent: String = source[line_start..]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let eol = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut annotations = Vec::new();
    match language {
        SourceLanguage::TypeScript => {
            let tail = &source[line_start..];
            let brace = body_brace(tail, language, symbol, target)?;
            let offset = line_start + brace + 1;
            let inner = format!("{indent}  ");
            let existing = leading_contract_lines(&tail[brace + 1..], "//@");
            for clause in &target.requires {
                let line = format!("{inner}//@ requires {clause}");
                if !existing.lines().any(|existing| existing == line.trim()) {
                    annotations.push(line);
                }
            }
            for clause in &target.ensures {
                let line = format!("{inner}//@ ensures {clause}");
                if !existing.lines().any(|existing| existing == line.trim()) {
                    annotations.push(line);
                }
            }
            Ok((!annotations.is_empty())
                .then(|| (offset, format!("{eol}{}", annotations.join(eol)))))
        }
        SourceLanguage::Java => {
            let existing = trailing_contract_lines(&source[..line_start], "//@");
            for clause in &target.requires {
                let line = format!("{indent}//@ requires {clause}");
                if !existing.lines().any(|existing| existing == line.trim()) {
                    annotations.push(line);
                }
            }
            for clause in &target.ensures {
                let line = format!("{indent}//@ ensures {clause}");
                if !existing.lines().any(|existing| existing == line.trim()) {
                    annotations.push(line);
                }
            }
            Ok((!annotations.is_empty())
                .then(|| (line_start, format!("{}{eol}", annotations.join(eol)))))
        }
        SourceLanguage::Rust => {
            let tail = &source[line_start..];
            let brace = body_brace(tail, language, symbol, target)?;
            let offset = line_start + brace;
            let clause_indent = format!("{indent}    ");
            let existing = &tail[..brace];
            for clause in &target.requires {
                let line = format!("{clause_indent}requires {clause},");
                if !existing
                    .lines()
                    .any(|existing| existing.trim() == line.trim())
                {
                    annotations.push(line);
                }
            }
            for clause in &target.ensures {
                let line = format!("{clause_indent}ensures {clause},");
                if !existing
                    .lines()
                    .any(|existing| existing.trim() == line.trim())
                {
                    annotations.push(line);
                }
            }
            Ok((!annotations.is_empty()).then(|| {
                (
                    offset,
                    format!("{eol}{}{eol}{indent}", annotations.join(eol)),
                )
            }))
        }
    }
}

/// Byte offset of the `{` that opens the function body, searching from the
/// start of the symbol's first line. Generic bounds, the parameter list,
/// strings and comments are stepped over so their braces are never mistaken
/// for the body.
fn body_brace(
    tail: &str,
    language: SourceLanguage,
    symbol: &DiscoveredSymbol,
    target: &ContractTarget,
) -> AuthoringResult<usize> {
    let bytes = tail.as_bytes();
    let malformed = |detail: &str| {
        path_error(
            AuthoringErrorCode::Malformed,
            &target.file,
            format!("symbol '{}' {detail}", target.symbol),
        )
    };

    let mut index = tail
        .find(symbol.name.as_str())
        .map_or(0, |start| start + symbol.name.len());
    index = skip_trivia(bytes, index);
    if bytes.get(index) == Some(&b'<') {
        index = match_generics(bytes, index, language)
            .ok_or_else(|| malformed("has an unterminated generic parameter list"))?;
    }
    let open =
        scan_to(bytes, index, language, b'(').ok_or_else(|| malformed("has no parameter list"))?;
    index = match_delimiter(bytes, open, language)
        .ok_or_else(|| malformed("has an unterminated parameter list"))?;

    loop {
        index = skip_trivia(bytes, index);
        match bytes.get(index) {
            None => return Err(malformed("has no function body")),
            Some(b';') => return Err(malformed("has no writable function body")),
            Some(b'{') => {
                let close = match_delimiter(bytes, index, language)
                    .ok_or_else(|| malformed("has an unterminated function body"))?;
                // A TypeScript return type can itself be an object literal
                // (`): { ok: boolean } {`); the body is what follows it.
                if language == SourceLanguage::TypeScript
                    && bytes.get(skip_trivia(bytes, close)) == Some(&b'{')
                {
                    index = skip_trivia(bytes, close);
                    continue;
                }
                return Ok(index);
            }
            Some(b'"' | b'\'' | b'`') => index = skip_quoted(bytes, index, language),
            Some(_) => index += 1,
        }
    }
}

/// Index of the next `target` byte that is real code, skipping over string
/// literals and comments.
fn scan_to(bytes: &[u8], mut index: usize, language: SourceLanguage, target: u8) -> Option<usize> {
    while let Some(byte) = bytes.get(index) {
        match *byte {
            byte if byte == target => return Some(index),
            b'"' | b'\'' | b'`' => index = skip_quoted(bytes, index, language),
            b'/' if matches!(bytes.get(index + 1), Some(b'/' | b'*')) => {
                index = skip_trivia(bytes, index);
            }
            _ => index += 1,
        }
    }
    None
}

fn skip_trivia(bytes: &[u8], mut index: usize) -> usize {
    loop {
        match bytes.get(index) {
            Some(byte) if byte.is_ascii_whitespace() => index += 1,
            Some(b'/') => match bytes.get(index + 1) {
                Some(b'/') => {
                    index += 2;
                    while bytes.get(index).is_some_and(|byte| *byte != b'\n') {
                        index += 1;
                    }
                }
                Some(b'*') => {
                    index += 2;
                    while index < bytes.len()
                        && !(bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/'))
                    {
                        index += 1;
                    }
                    index = bytes.len().min(index + 2);
                }
                _ => return index,
            },
            _ => return index,
        }
    }
}

/// Index just past the closing quote of the literal starting at `index`. A
/// Rust lifetime (`'a`) is not a literal, so it advances a single byte.
fn skip_quoted(bytes: &[u8], index: usize, language: SourceLanguage) -> usize {
    let quote = bytes[index];
    if quote == b'\'' && language == SourceLanguage::Rust && !is_char_literal(bytes, index) {
        return index + 1;
    }
    let mut cursor = index + 1;
    while let Some(byte) = bytes.get(cursor) {
        match *byte {
            b'\\' => cursor += 2,
            byte if byte == quote => return cursor + 1,
            _ => cursor += 1,
        }
    }
    cursor
}

fn is_char_literal(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index + 1) {
        Some(b'\\') => true,
        Some(_) => bytes.get(index + 2) == Some(&b'\''),
        None => false,
    }
}

/// Index just past the bracket matching the one at `open`.
fn match_delimiter(bytes: &[u8], open: usize, language: SourceLanguage) -> Option<usize> {
    let opening = *bytes.get(open)?;
    let closing = match opening {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return None,
    };
    let mut depth = 0_usize;
    let mut index = open;
    while let Some(byte) = bytes.get(index) {
        match *byte {
            b'"' | b'\'' | b'`' => {
                index = skip_quoted(bytes, index, language);
                continue;
            }
            b'/' if matches!(bytes.get(index + 1), Some(b'/' | b'*')) => {
                index = skip_trivia(bytes, index);
                continue;
            }
            byte if byte == opening => depth += 1,
            byte if byte == closing => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Index just past the `>` closing the generic list at `open`. Arrow and
/// return tokens (`=>`, `->`) are consumed whole so their `>` does not read
/// as a closing bracket.
fn match_generics(bytes: &[u8], open: usize, language: SourceLanguage) -> Option<usize> {
    let mut depth = 0_usize;
    let mut index = open;
    while let Some(byte) = bytes.get(index) {
        match *byte {
            b'"' | b'\'' | b'`' => {
                index = skip_quoted(bytes, index, language);
            }
            b'=' | b'-' if bytes.get(index + 1) == Some(&b'>') => index += 2,
            b'(' => index = match_delimiter(bytes, index, language)?,
            b'<' => {
                depth += 1;
                index += 1;
            }
            b'>' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
    None
}

fn leading_contract_lines(source: &str, marker: &str) -> String {
    source
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| line.trim_start().starts_with(marker))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

fn trailing_contract_lines(source: &str, marker: &str) -> String {
    let mut lines = source
        .lines()
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| line.trim_start().starts_with(marker))
        .map(str::trim)
        .collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

fn line_start_offset(source: &str, one_based_line: usize) -> usize {
    if one_based_line <= 1 {
        return 0;
    }
    source
        .match_indices('\n')
        .nth(one_based_line - 2)
        .map_or(0, |(offset, _)| offset + 1)
}

fn read_regular_file(root: &CanonicalRoot, relative: &str) -> AuthoringResult<Vec<u8>> {
    let path = root.resolve(relative)?;
    let metadata = fs::symlink_metadata(&path).map_err(|err| {
        path_error(
            if err.kind() == std::io::ErrorKind::NotFound {
                AuthoringErrorCode::NotFound
            } else {
                AuthoringErrorCode::Io
            },
            relative,
            format!("cannot read file: {err}"),
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(path_error(
            AuthoringErrorCode::UnsafePath,
            relative,
            "path must refer to a regular file",
        ));
    }
    fs::read(path).map_err(|err| {
        path_error(
            AuthoringErrorCode::Io,
            relative,
            format!("cannot read file: {err}"),
        )
    })
}

fn apply_writes(
    root: &CanonicalRoot,
    writes: Vec<(String, Vec<u8>)>,
    options: &MutationOptions,
) -> AuthoringResult<MutationOutput> {
    let mut output = MutationOutput::default();
    let mut prepared = Vec::new();
    for (relative, bytes) in writes {
        let path = root.resolve(&relative)?;
        let existing = if path.exists() {
            Some(read_regular_file(root, &relative)?)
        } else {
            None
        };
        if let Some(expected) = options.expected_sha256.get(&relative) {
            let actual = existing.as_ref().map(|bytes| sha256(bytes));
            if actual.as_deref() != Some(expected.as_str()) {
                return Err(path_error(
                    AuthoringErrorCode::StaleContent,
                    &relative,
                    "file changed since it was read",
                ));
            }
        } else if existing.is_some() && existing.as_ref() != Some(&bytes) {
            return Err(path_error(
                AuthoringErrorCode::StaleContent,
                &relative,
                "expected SHA-256 is required when updating an existing file",
            ));
        }
        let action = match &existing {
            None => {
                output.created.push(relative.clone());
                FileMutationAction::Created
            }
            Some(current) if *current == bytes => {
                output.unchanged.push(relative.clone());
                FileMutationAction::Unchanged
            }
            Some(_) => {
                output.updated.push(relative.clone());
                FileMutationAction::Updated
            }
        };
        output.files.push(FileMutation {
            path: relative.clone(),
            action,
            before_sha256: existing.as_ref().map(|current| sha256(current)),
            after_sha256: sha256(&bytes),
            proposed_content: (options.mode == MutationMode::Preview)
                .then(|| String::from_utf8_lossy(&bytes).into_owned()),
        });
        if existing.as_ref() != Some(&bytes) {
            prepared.push((relative, path, bytes));
        }
    }
    if options.mode == MutationMode::Apply {
        for (relative, path, bytes) in prepared {
            atomic_write(&path, &bytes).map_err(|err| {
                path_error(
                    AuthoringErrorCode::Io,
                    relative,
                    format!("atomic write failed: {err}"),
                )
            })?;
        }
    }
    Ok(output)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "file has no parent")
    })?;
    fs::create_dir_all(parent)?;
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".forall-write-{}-{sequence}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn preserve_eol(existing: &[u8], rendered: &str) -> Vec<u8> {
    if existing.windows(2).any(|window| window == b"\r\n") {
        rendered.replace('\n', "\r\n").into_bytes()
    } else {
        rendered.as_bytes().to_vec()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests;
