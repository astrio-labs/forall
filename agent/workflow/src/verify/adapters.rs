use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mapping::schema::{IssueSeverity, Requirement, VerifyIssue};
use crate::mapping::validate::verified_files_from_requirements;
use crate::paths::{verify_adapters_path, verify_adapters_read_path};

pub const VERIFIED_CONFIG_REL: &str = ".forall/verify/adapters.yaml";

/// Proof backend id for LemmaScript → Dafny (TypeScript only in M2).
pub const PROOF_LEMMASCRIPT_DAFNY: &str = "lemmascript-dafny";
pub const PROOF_VERUS: &str = "verus";
pub const PROOF_OPENJML: &str = "openjml";
pub const PROOF_FRAMAC: &str = "framac";
pub const PROOF_NONE: &str = "none";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AdapterConfig {
    pub proof: String,
    #[serde(default)]
    pub extensions: Vec<String>,
}

impl AdapterConfig {
    pub fn supports_proof(&self) -> bool {
        !self.proof.is_empty() && self.proof != PROOF_NONE
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct VerifiedConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default)]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_js: Option<String>,
    #[serde(default = "default_adapters")]
    pub adapters: BTreeMap<String, AdapterConfig>,
}

fn default_backend() -> String {
    "dafny".to_string()
}

pub fn default_adapters() -> BTreeMap<String, AdapterConfig> {
    BTreeMap::from([
        (
            "typescript".to_string(),
            AdapterConfig {
                proof: PROOF_LEMMASCRIPT_DAFNY.to_string(),
                extensions: vec!["ts".to_string(), "tsx".to_string()],
            },
        ),
        (
            "python".to_string(),
            AdapterConfig {
                proof: PROOF_NONE.to_string(),
                extensions: vec!["py".to_string()],
            },
        ),
        (
            "rust".to_string(),
            AdapterConfig {
                proof: PROOF_VERUS.to_string(),
                extensions: vec!["rs".to_string()],
            },
        ),
        (
            "java".to_string(),
            AdapterConfig {
                proof: PROOF_OPENJML.to_string(),
                extensions: vec!["java".to_string()],
            },
        ),
        (
            "c".to_string(),
            AdapterConfig {
                proof: PROOF_FRAMAC.to_string(),
                extensions: vec!["c".to_string()],
            },
        ),
    ])
}

impl Default for VerifiedConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            strict: false,
            cli_js: None,
            adapters: default_adapters(),
        }
    }
}

impl VerifiedConfig {
    pub fn merge_adapter_defaults(&mut self) {
        for (name, default) in default_adapters() {
            self.adapters.entry(name).or_insert(default);
        }
        for adapter in self.adapters.values_mut() {
            if adapter.extensions.is_empty() {
                adapter.extensions = default_adapters()
                    .values()
                    .find(|candidate| candidate.proof == adapter.proof)
                    .map(|candidate| candidate.extensions.clone())
                    .unwrap_or_default();
            }
        }
    }

    pub fn adapter_for_file(&self, rel_path: &str) -> Option<(&str, &AdapterConfig)> {
        let ext = file_extension(rel_path)?;
        self.adapters
            .iter()
            .find(|(_, adapter)| adapter.extensions.iter().any(|e| e == ext))
            .map(|(name, adapter)| (name.as_str(), adapter))
    }

    pub fn supports_proof_for_file(&self, rel_path: &str) -> bool {
        self.adapter_for_file(rel_path)
            .is_some_and(|(_, adapter)| adapter.supports_proof())
    }
}

pub fn verified_config_path(root: &Path) -> PathBuf {
    verify_adapters_path(root)
}

pub fn load_verified_config(root: &Path) -> VerifiedConfig {
    let path = verify_adapters_read_path(root);
    let mut config = read_verified_config_file(&path);
    config.merge_adapter_defaults();
    config
}

fn read_verified_config_file(path: &Path) -> VerifiedConfig {
    if path.is_file() {
        fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_yaml::from_str(&raw).ok())
            .unwrap_or_default()
    } else {
        VerifiedConfig::default()
    }
}

pub fn save_verified_config(root: &Path, config: &VerifiedConfig) -> std::io::Result<()> {
    let path = verified_config_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_yaml::to_string(config)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    fs::write(path, raw)
}

/// Persist the default Verus adapter when a legacy on-disk config omits `adapters.rust`.
pub fn ensure_rust_adapter_persisted(root: &Path) -> std::io::Result<bool> {
    let path = verified_config_path(root);
    if !path.is_file() {
        return Ok(false);
    }
    let mut config = read_verified_config_file(&path);
    let had_rust = config
        .adapters
        .get("rust")
        .is_some_and(|adapter| adapter.proof == PROOF_VERUS);
    config.merge_adapter_defaults();
    if config
        .adapters
        .get("rust")
        .is_some_and(|adapter| adapter.proof != PROOF_VERUS)
    {
        config.adapters.insert(
            "rust".to_string(),
            default_adapters()
                .remove("rust")
                .unwrap_or_else(|| panic!("missing rust default adapter")),
        );
    }
    if had_rust {
        return Ok(false);
    }
    save_verified_config(root, &config)?;
    Ok(true)
}

/// Persist the default OpenJML adapter when a legacy on-disk config omits `adapters.java`.
pub fn ensure_java_adapter_persisted(root: &Path) -> std::io::Result<bool> {
    let path = verified_config_path(root);
    if !path.is_file() {
        return Ok(false);
    }
    let mut config = read_verified_config_file(&path);
    let had_java = config
        .adapters
        .get("java")
        .is_some_and(|adapter| adapter.proof == PROOF_OPENJML);
    config.merge_adapter_defaults();
    if config
        .adapters
        .get("java")
        .is_some_and(|adapter| adapter.proof != PROOF_OPENJML)
    {
        config.adapters.insert(
            "java".to_string(),
            default_adapters()
                .remove("java")
                .unwrap_or_else(|| panic!("missing java default adapter")),
        );
    }
    if had_java {
        return Ok(false);
    }
    save_verified_config(root, &config)?;
    Ok(true)
}

/// Persist the default Frama-C adapter when a legacy on-disk config omits `adapters.c`.
pub fn ensure_c_adapter_persisted(root: &Path) -> std::io::Result<bool> {
    let path = verified_config_path(root);
    if !path.is_file() {
        return Ok(false);
    }
    let mut config = read_verified_config_file(&path);
    let had_c = config
        .adapters
        .get("c")
        .is_some_and(|adapter| adapter.proof == PROOF_FRAMAC);
    config.merge_adapter_defaults();
    if config
        .adapters
        .get("c")
        .is_some_and(|adapter| adapter.proof != PROOF_FRAMAC)
    {
        config.adapters.insert(
            "c".to_string(),
            default_adapters()
                .remove("c")
                .unwrap_or_else(|| panic!("missing c default adapter")),
        );
    }
    if had_c {
        return Ok(false);
    }
    save_verified_config(root, &config)?;
    Ok(true)
}

pub fn default_verified_config_yaml(cli_exe: &str) -> String {
    format!(
        "backend: dafny\nstrict: false\ncli_js: {cli_exe:?}\nadapters:\n  typescript:\n    proof: {PROOF_LEMMASCRIPT_DAFNY}\n    extensions: [ts, tsx]\n  python:\n    proof: {PROOF_NONE}\n    extensions: [py]\n  rust:\n    proof: {PROOF_VERUS}\n    extensions: [rs]\n  java:\n    proof: {PROOF_OPENJML}\n    extensions: [java]\n  c:\n    proof: {PROOF_FRAMAC}\n    extensions: [c]\n"
    )
}

pub fn proof_eligible_files(
    requirements: &[Requirement],
    config: &VerifiedConfig,
    mapping_version: u32,
) -> Vec<String> {
    verified_files_from_requirements(requirements, mapping_version)
        .into_iter()
        .filter(|file| config.supports_proof_for_file(file))
        .collect()
}

pub fn validate_adapter_requirements(
    requirements: &[Requirement],
    config: &VerifiedConfig,
) -> Vec<VerifyIssue> {
    let mut issues = Vec::new();

    for req in requirements {
        if !req.verified {
            continue;
        }
        let Some(code) = &req.code else {
            continue;
        };
        if config.supports_proof_for_file(&code.file) {
            continue;
        }

        let ext = file_extension(&code.file).unwrap_or("unknown");
        let message = match config.adapter_for_file(&code.file) {
            Some((adapter_name, adapter)) if !adapter.supports_proof() => format!(
                "requirement '{}' marks verified: true but adapter '{adapter_name}' has proof: {} for .{ext} files — set verified: false or use property_tested",
                req.id, adapter.proof
            ),
            Some((adapter_name, _)) => format!(
                "requirement '{}' marks verified: true but adapter '{adapter_name}' does not run proofs for .{ext} files",
                req.id
            ),
            None => format!(
                "requirement '{}' marks verified: true but no verification adapter is registered for .{ext} files — add adapters in {VERIFIED_CONFIG_REL}",
                req.id
            ),
        };

        issues.push(VerifyIssue {
            severity: IssueSeverity::Critical,
            phase: "mapping".to_string(),
            requirement_id: Some(req.id.clone()),
            message,
            file: Some(code.file.clone()),
            counterexample: None,
            pbt: None,
            proof_detail: None,
        });
    }

    issues
}

pub fn file_extension(rel_path: &str) -> Option<&str> {
    Path::new(rel_path).extension().and_then(|ext| ext.to_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_supports_typescript_proof_not_python() {
        let config = VerifiedConfig::default();
        assert!(config.supports_proof_for_file("src/foo.ts"));
        assert!(config.supports_proof_for_file("src/foo.rs"));
        assert!(config.supports_proof_for_file("src/foo.java"));
        assert!(config.supports_proof_for_file("src/foo.c"));
        assert!(!config.supports_proof_for_file("src/foo.py"));
    }

    #[test]
    fn default_config_registers_openjml_for_java() {
        let config = VerifiedConfig::default();
        let (_, adapter) = config.adapter_for_file("src/Ledger.java").unwrap();
        assert_eq!(adapter.proof, PROOF_OPENJML);
    }

    #[test]
    fn default_config_registers_framac_for_c() {
        let config = VerifiedConfig::default();
        let (_, adapter) = config.adapter_for_file("src/ledger.c").unwrap();
        assert_eq!(adapter.proof, PROOF_FRAMAC);
    }

    #[test]
    fn verified_true_python_emits_critical_adapter_issue() {
        let config = VerifiedConfig::default();
        let requirements = vec![Requirement {
            id: "py-req".to_string(),
            capability: "demo".to_string(),
            requirement: "demo".to_string(),
            verified: true,
            property_tested: false,
            property: None,
            code: Some(crate::mapping::schema::CodeRef {
                file: "src/demo.py".to_string(),
                symbols: vec!["demo".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        }];
        let issues = validate_adapter_requirements(&requirements, &config);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, IssueSeverity::Critical);
        assert!(issues[0].message.contains("proof: none"));
        assert!(proof_eligible_files(&requirements, &config, 1).is_empty());
    }

    #[test]
    fn verified_false_python_skips_proof_files() {
        let config = VerifiedConfig::default();
        let requirements = vec![Requirement {
            id: "py-req".to_string(),
            capability: "demo".to_string(),
            requirement: "demo".to_string(),
            verified: false,
            property_tested: true,
            property: Some(crate::mapping::schema::PropertyRef {
                file: ".forall/scenarios/demo.property.py".to_string(),
                symbol: Some("demo".to_string()),
            }),
            code: Some(crate::mapping::schema::CodeRef {
                file: "src/demo.py".to_string(),
                symbols: vec!["demo".to_string()],
            }),
            contract: None,
            claimcheck: None,
            scenarios: None,
        }];
        assert!(validate_adapter_requirements(&requirements, &config).is_empty());
        assert!(proof_eligible_files(&requirements, &config, 1).is_empty());
    }

    #[test]
    fn ensure_rust_adapter_persisted_updates_legacy_config() {
        let dir = tempdir().unwrap();
        let path = verified_config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "backend: dafny\nstrict: false\ncli_js: \"/tmp/forall\"\nadapters:\n  typescript:\n    proof: lemmascript-dafny\n    extensions: [ts, tsx]\n  python:\n    proof: none\n    extensions: [py]\n",
        )
        .unwrap();
        assert!(ensure_rust_adapter_persisted(dir.path()).unwrap());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("rust:"));
        assert!(raw.contains("verus"));
        let config = load_verified_config(dir.path());
        assert_eq!(
            config.adapter_for_file("src/foo.rs").unwrap().1.proof,
            PROOF_VERUS
        );
    }

    #[test]
    fn load_verified_config_merges_defaults_for_legacy_file() {
        let dir = tempdir().unwrap();
        let path = verified_config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "backend: dafny\nstrict: false\ncli_js: \"/tmp/forall\"\n",
        )
        .unwrap();
        let config = load_verified_config(dir.path());
        assert!(config.adapters.contains_key("typescript"));
        assert!(config.adapters.contains_key("python"));
        assert!(config.adapters.contains_key("rust"));
        assert!(config.adapters.contains_key("java"));
    }

    #[test]
    fn ensure_java_adapter_persisted_updates_legacy_config() {
        let dir = tempdir().unwrap();
        let path = verified_config_path(dir.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "backend: dafny\nstrict: false\ncli_js: \"/tmp/forall\"\nadapters:\n  typescript:\n    proof: lemmascript-dafny\n    extensions: [ts, tsx]\n  python:\n    proof: none\n    extensions: [py]\n  rust:\n    proof: verus\n    extensions: [rs]\n",
        )
        .unwrap();
        assert!(ensure_java_adapter_persisted(dir.path()).unwrap());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("java:"));
        assert!(raw.contains("openjml"));
        let config = load_verified_config(dir.path());
        assert_eq!(
            config.adapter_for_file("src/Foo.java").unwrap().1.proof,
            PROOF_OPENJML
        );
    }
}
