use std::fs;
use std::path::Path;

use include_dir::DirEntry;

use crate::agent_home::resolve_forall_home;
use crate::bundled::{BUNDLED_SCHEMA_DIR, bundled_text};
use crate::paths::ensure_dir;

fn parse_skill_version(content: &str) -> Option<String> {
    let frontmatter = content.strip_prefix("---")?;
    let end = frontmatter.find("\n---")?;
    for line in frontmatter[..end].lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version:") {
            return Some(rest.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn version_is_newer(installed: &str, bundled: &str) -> bool {
    fn parse(v: &str) -> Option<(u32, u32)> {
        let mut parts = v.split('.');
        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
    }
    match (parse(installed), parse(bundled)) {
        (Some((a_maj, a_min)), Some((b_maj, b_min))) => (b_maj, b_min) > (a_maj, a_min),
        _ => bundled != installed,
    }
}

fn should_install_skill(dest: &Path, bundled_content: &str) -> bool {
    if !dest.is_file() {
        return true;
    }
    let Ok(installed) = fs::read_to_string(dest) else {
        return true;
    };
    let Some(bundled_ver) = parse_skill_version(bundled_content) else {
        return false;
    };
    let installed_ver = parse_skill_version(&installed).unwrap_or_else(|| "0.0".to_string());
    version_is_newer(&installed_ver, &bundled_ver)
}

/// Install bundled Forall workflow skills into `$FORALL_HOME/skills/`.
pub fn install_workflow_skills() -> std::io::Result<Vec<String>> {
    let Some(skills_src) = BUNDLED_SCHEMA_DIR.get_dir("skills") else {
        return Ok(Vec::new());
    };

    let skills_dest = resolve_forall_home().join("skills");
    ensure_dir(&skills_dest)?;

    let mut installed = Vec::new();
    for entry in skills_src.entries() {
        let DirEntry::Dir(skill_dir) = entry else {
            continue;
        };
        let Some(skill_name) = skill_dir.path().file_name() else {
            continue;
        };
        let content = bundled_text(skill_dir.path().join("SKILL.md"))?;
        let dest_dir = skills_dest.join(skill_name);
        let dest_skill = dest_dir.join("SKILL.md");
        if !should_install_skill(&dest_skill, content) {
            continue;
        }
        ensure_dir(&dest_dir)?;
        fs::write(&dest_skill, content.as_bytes())?;
        installed.push(skill_name.to_string_lossy().into_owned());
    }
    installed.sort();
    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_newer_compares_semver_like_pairs() {
        assert!(version_is_newer("1.0", "1.1"));
        assert!(!version_is_newer("1.1", "1.0"));
        assert!(version_is_newer("1.0", "2.0"));
    }
}
