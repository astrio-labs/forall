use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths::{ensure_dir, specs_dir};
use crate::specs::requirement_blocks::{
    RequirementBlock, build_spec_skeleton, extract_requirements_section,
    infer_delta_plan_from_full_spec, normalize_requirement_name, parse_delta_spec,
};
use crate::workflow::paths::change_dir;

#[derive(Debug, Clone)]
pub struct SpecUpdate {
    pub source: PathBuf,
    pub target: PathBuf,
    pub exists: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyResult {
    pub capability: String,
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub renamed: usize,
}

#[derive(Debug, Clone)]
pub struct SpecsApplyOutput {
    pub change_name: String,
    pub capabilities: Vec<ApplyResult>,
    pub totals: ApplyTotals,
    pub no_changes: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyTotals {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub renamed: usize,
}

pub struct BuildOptions {
    pub lenient: bool,
}

pub struct ApplyOptions {
    pub dry_run: bool,
    pub silent: bool,
    pub lenient: bool,
}

pub fn find_spec_updates(root: &Path, change_name: &str) -> Vec<SpecUpdate> {
    let change_specs_dir = change_dir(root, change_name).join("specs");
    let main_specs_dir = specs_dir(root);
    let mut updates = Vec::new();

    let Ok(entries) = fs::read_dir(&change_specs_dir) else {
        return updates;
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let source = entry.path().join("spec.md");
        if !source.is_file() {
            continue;
        }
        let capability = entry.file_name().to_string_lossy().into_owned();
        let target = main_specs_dir.join(capability).join("spec.md");
        let exists = target.is_file();
        updates.push(SpecUpdate {
            source,
            target,
            exists,
        });
    }

    updates
}

pub fn build_updated_spec(
    update: &SpecUpdate,
    change_name: &str,
    options: &BuildOptions,
) -> Result<(String, ApplyResult), String> {
    let change_content = fs::read_to_string(&update.source).map_err(|e| e.to_string())?;
    let mut plan = parse_delta_spec(&change_content);
    let capability = update
        .target
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let has_any_delta = !plan.added.is_empty()
        || !plan.modified.is_empty()
        || !plan.removed.is_empty()
        || !plan.renamed.is_empty();

    let (target_content, is_new_spec) = if update.exists {
        (
            fs::read_to_string(&update.target).map_err(|e| e.to_string())?,
            false,
        )
    } else {
        if !plan.modified.is_empty() || !plan.renamed.is_empty() {
            return Err(format!(
                "{capability}: target spec does not exist; only ADDED requirements are allowed for new specs."
            ));
        }
        (build_spec_skeleton(&capability, change_name), true)
    };

    if !has_any_delta
        && let Some(inferred) = infer_delta_plan_from_full_spec(&change_content, &target_content)
    {
        plan = inferred;
    }

    let has_any_delta_after = !plan.added.is_empty()
        || !plan.modified.is_empty()
        || !plan.removed.is_empty()
        || !plan.renamed.is_empty();
    if !has_any_delta_after {
        return Err(format!(
            "Delta parsing found no operations for {capability}. \
             Provide ADDED/MODIFIED/REMOVED/RENAMED sections in change spec, \
             or include new ### Requirement blocks."
        ));
    }

    let parts = extract_requirements_section(&target_content);
    let mut name_to_block: HashMap<String, RequirementBlock> = parts
        .body_blocks
        .iter()
        .map(|b| (normalize_requirement_name(&b.name), b.clone()))
        .collect();

    for r in &plan.renamed {
        let from = normalize_requirement_name(&r.from);
        let to = normalize_requirement_name(&r.to);
        if !name_to_block.contains_key(&from) {
            return Err(format!(
                "{capability} RENAMED failed: \"{}\" not found",
                r.from
            ));
        }
        if name_to_block.contains_key(&to) {
            return Err(format!(
                "{capability} RENAMED failed: \"{}\" already exists",
                r.to
            ));
        }
        let block = name_to_block
            .remove(&from)
            .ok_or_else(|| format!("{capability} RENAMED failed: \"{}\" not found", r.from))?;
        let new_header = format!("### Requirement: {to}");
        let mut raw_lines: Vec<String> = block.raw.split('\n').map(str::to_string).collect();
        if !raw_lines.is_empty() {
            raw_lines[0] = new_header.clone();
        }
        name_to_block.insert(
            to.clone(),
            RequirementBlock {
                header_line: new_header,
                name: to,
                raw: raw_lines.join("\n"),
            },
        );
    }

    for name in &plan.removed {
        let key = normalize_requirement_name(name);
        if !name_to_block.contains_key(&key) {
            if !is_new_spec {
                return Err(format!("{capability} REMOVED failed: \"{name}\" not found"));
            }
            continue;
        }
        name_to_block.remove(&key);
    }

    for mod_block in &plan.modified {
        let key = normalize_requirement_name(&mod_block.name);
        if !name_to_block.contains_key(&key) {
            return Err(format!(
                "{capability} MODIFIED failed: \"{}\" not found",
                mod_block.name
            ));
        }
        name_to_block.insert(key, mod_block.clone());
    }

    for add in &plan.added {
        let key = normalize_requirement_name(&add.name);
        if name_to_block.contains_key(&key) {
            if options.lenient {
                continue;
            }
            return Err(format!(
                "{capability} ADDED failed: \"{}\" already exists",
                add.name
            ));
        }
        name_to_block.insert(key, add.clone());
    }

    let mut kept_order = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for block in &parts.body_blocks {
        let key = normalize_requirement_name(&block.name);
        if let Some(replacement) = name_to_block.get(&key) {
            kept_order.push(replacement.clone());
            seen.insert(key);
        }
    }
    for (key, block) in &name_to_block {
        if !seen.contains(key) {
            kept_order.push(block.clone());
        }
    }

    let mut req_body_parts = Vec::new();
    if !parts.preamble.trim().is_empty() {
        req_body_parts.push(parts.preamble.trim_end().to_string());
    }
    req_body_parts.extend(kept_order.iter().map(|b| b.raw.clone()));
    let req_body = req_body_parts.join("\n\n").trim_end().to_string();

    let mut rebuilt_parts = Vec::new();
    let before = parts.before.trim_end();
    if !before.is_empty() {
        rebuilt_parts.push(before.to_string());
    }
    rebuilt_parts.push(parts.header_line.clone());
    rebuilt_parts.push(req_body);
    rebuilt_parts.push(parts.after.clone());
    let mut rebuilt = rebuilt_parts.join("\n");
    rebuilt = regex_replace_triple_newline(&rebuilt);

    let added_count = plan
        .added
        .iter()
        .filter(|a| {
            !options.lenient
                || !parts.body_blocks.iter().any(|b| {
                    normalize_requirement_name(&b.name) == normalize_requirement_name(&a.name)
                })
        })
        .count();

    Ok((
        rebuilt,
        ApplyResult {
            capability,
            added: added_count,
            modified: plan.modified.len(),
            removed: plan.removed.len(),
            renamed: plan.renamed.len(),
        },
    ))
}

fn regex_replace_triple_newline(text: &str) -> String {
    let re = regex::Regex::new(r"\n{3,}")
        .unwrap_or_else(|err| panic!("invalid triple-newline regex: {err}"));
    re.replace_all(text, "\n\n").into_owned()
}

pub fn apply_specs(
    root: &Path,
    change_name: &str,
    options: &ApplyOptions,
) -> Result<SpecsApplyOutput, String> {
    let src = change_dir(root, change_name);
    if !src.is_dir() {
        return Err(format!("Change '{change_name}' not found."));
    }

    let spec_updates = find_spec_updates(root, change_name);
    if spec_updates.is_empty() {
        return Ok(SpecsApplyOutput {
            change_name: change_name.to_string(),
            capabilities: Vec::new(),
            totals: ApplyTotals::default(),
            no_changes: true,
        });
    }

    let build_opts = BuildOptions {
        lenient: options.lenient,
    };
    let mut prepared = Vec::new();
    for update in &spec_updates {
        let (rebuilt, counts) = build_updated_spec(update, change_name, &build_opts)?;
        prepared.push((update.clone(), rebuilt, counts));
    }

    let mut capabilities = Vec::new();
    let mut totals = ApplyTotals::default();

    for (update, rebuilt, counts) in prepared {
        if !options.dry_run {
            if let Some(parent) = update.target.parent() {
                ensure_dir(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&update.target, &rebuilt).map_err(|e| e.to_string())?;
        }

        if !options.silent {
            let rel = update
                .target
                .strip_prefix(root)
                .unwrap_or(&update.target)
                .display();
            println!(
                "{} {rel}:",
                if options.dry_run {
                    "Would apply"
                } else {
                    "Applied"
                }
            );
            if counts.added > 0 {
                println!("  + {} added", counts.added);
            }
            if counts.modified > 0 {
                println!("  ~ {} modified", counts.modified);
            }
            if counts.removed > 0 {
                println!("  - {} removed", counts.removed);
            }
            if counts.renamed > 0 {
                println!("  → {} renamed", counts.renamed);
            }
        }

        totals.added += counts.added;
        totals.modified += counts.modified;
        totals.removed += counts.removed;
        totals.renamed += counts.renamed;
        capabilities.push(counts);
    }

    Ok(SpecsApplyOutput {
        change_name: change_name.to_string(),
        capabilities,
        totals,
        no_changes: false,
    })
}
