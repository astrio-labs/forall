use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequirementBlock {
    pub header_line: String,
    pub name: String,
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct RequirementsSectionParts {
    pub before: String,
    pub header_line: String,
    pub preamble: String,
    pub body_blocks: Vec<RequirementBlock>,
    pub after: String,
}

#[derive(Debug, Clone, Default)]
pub struct DeltaPlan {
    pub added: Vec<RequirementBlock>,
    pub modified: Vec<RequirementBlock>,
    pub removed: Vec<String>,
    pub renamed: Vec<RenamePair>,
}

#[derive(Debug, Clone)]
pub struct RenamePair {
    pub from: String,
    pub to: String,
}

static REQUIREMENT_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^###\s*Requirement:\s*(.+)\s*$")
        .unwrap_or_else(|err| panic!("invalid requirement header regex: {err}"))
});
static REQUIREMENTS_SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^##\s+Requirements\s*$")
        .unwrap_or_else(|err| panic!("invalid requirements section regex: {err}"))
});
static H2_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^##\s+").unwrap_or_else(|err| panic!("invalid h2 header regex: {err}"))
});

pub fn normalize_requirement_name(name: &str) -> String {
    name.trim().to_string()
}

fn normalize_line_endings(content: &str) -> String {
    content.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn extract_requirements_section(content: &str) -> RequirementsSectionParts {
    let normalized = normalize_line_endings(content);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let req_header_index = lines.iter().position(|l| REQUIREMENTS_SECTION.is_match(l));

    let Some(req_header_index) = req_header_index else {
        let before = content.trim_end();
        return RequirementsSectionParts {
            before: if before.is_empty() {
                String::new()
            } else {
                format!("{before}\n\n")
            },
            header_line: "## Requirements".to_string(),
            preamble: String::new(),
            body_blocks: Vec::new(),
            after: "\n".to_string(),
        };
    };

    let end_index = lines
        .iter()
        .enumerate()
        .skip(req_header_index + 1)
        .find(|(_, line)| H2_HEADER.is_match(line))
        .map_or(lines.len(), |(i, _)| i);

    let before = lines[..req_header_index].join("\n");
    let header_line = lines[req_header_index].to_string();
    let section_body_lines = &lines[req_header_index + 1..end_index];

    let mut blocks = Vec::new();
    let mut cursor = 0usize;
    let mut preamble_lines = Vec::new();

    while cursor < section_body_lines.len()
        && !REQUIREMENT_HEADER.is_match(section_body_lines[cursor])
    {
        preamble_lines.push(section_body_lines[cursor].to_string());
        cursor += 1;
    }

    while cursor < section_body_lines.len() {
        let header_line_candidate = section_body_lines[cursor];
        let Some(header_match) = REQUIREMENT_HEADER.captures(header_line_candidate) else {
            cursor += 1;
            continue;
        };
        let name = normalize_requirement_name(&header_match[1]);
        cursor += 1;
        let mut body_lines = vec![header_line_candidate.to_string()];
        while cursor < section_body_lines.len()
            && !REQUIREMENT_HEADER.is_match(section_body_lines[cursor])
            && !H2_HEADER.is_match(section_body_lines[cursor])
        {
            body_lines.push(section_body_lines[cursor].to_string());
            cursor += 1;
        }
        blocks.push(RequirementBlock {
            header_line: header_line_candidate.to_string(),
            name,
            raw: body_lines.join("\n").trim_end().to_string(),
        });
    }

    let after = lines[end_index..].join("\n");
    RequirementsSectionParts {
        before: {
            let trimmed = before.trim_end();
            if trimmed.is_empty() {
                before
            } else {
                format!("{trimmed}\n")
            }
        },
        header_line,
        preamble: preamble_lines.join("\n").trim_end().to_string(),
        body_blocks: blocks,
        after: if after.starts_with('\n') {
            after
        } else {
            format!("\n{after}")
        },
    }
}

pub fn parse_delta_spec(content: &str) -> DeltaPlan {
    let normalized = normalize_line_endings(content);
    let sections = split_top_level_sections(&normalized);
    DeltaPlan {
        added: parse_requirement_blocks_from_section(
            &get_section_case_insensitive(&sections, "ADDED Requirements").body,
        ),
        modified: parse_requirement_blocks_from_section(
            &get_section_case_insensitive(&sections, "MODIFIED Requirements").body,
        ),
        removed: parse_removed_names(
            &get_section_case_insensitive(&sections, "REMOVED Requirements").body,
        ),
        renamed: parse_renamed_pairs(
            &get_section_case_insensitive(&sections, "RENAMED Requirements").body,
        ),
    }
}

fn split_top_level_sections(content: &str) -> std::collections::HashMap<String, String> {
    let lines: Vec<&str> = content.split('\n').collect();
    let mut result = std::collections::HashMap::new();
    let mut indices = Vec::new();
    let h2 = Regex::new(r"^(##)\s+(.+)$")
        .unwrap_or_else(|err| panic!("invalid section header regex: {err}"));
    for (i, line) in lines.iter().enumerate() {
        if let Some(caps) = h2.captures(line) {
            indices.push((caps[2].trim().to_string(), i));
        }
    }
    for (i, (title, start)) in indices.iter().enumerate() {
        let end = indices
            .get(i + 1)
            .map(|(_, idx)| *idx)
            .unwrap_or(lines.len());
        result.insert(title.clone(), lines[*start + 1..end].join("\n"));
    }
    result
}

struct SectionBody {
    body: String,
}

fn get_section_case_insensitive(
    sections: &std::collections::HashMap<String, String>,
    desired: &str,
) -> SectionBody {
    let target = desired.to_lowercase();
    for (title, body) in sections {
        if title.to_lowercase() == target {
            return SectionBody { body: body.clone() };
        }
    }
    SectionBody {
        body: String::new(),
    }
}

fn parse_requirement_blocks_from_section(section_body: &str) -> Vec<RequirementBlock> {
    if section_body.is_empty() {
        return Vec::new();
    }
    let normalized = normalize_line_endings(section_body);
    let lines: Vec<&str> = normalized.split('\n').collect();
    let mut blocks = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        while i < lines.len() && !REQUIREMENT_HEADER.is_match(lines[i]) {
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        let header_line = lines[i].to_string();
        let Some(caps) = REQUIREMENT_HEADER.captures(&header_line) else {
            i += 1;
            continue;
        };
        let name = normalize_requirement_name(&caps[1]);
        let mut buf = vec![header_line.clone()];
        i += 1;
        while i < lines.len()
            && !REQUIREMENT_HEADER.is_match(lines[i])
            && !H2_HEADER.is_match(lines[i])
        {
            buf.push(lines[i].to_string());
            i += 1;
        }
        blocks.push(RequirementBlock {
            header_line,
            name,
            raw: buf.join("\n").trim_end().to_string(),
        });
    }
    blocks
}

fn parse_removed_names(section_body: &str) -> Vec<String> {
    if section_body.is_empty() {
        return Vec::new();
    }
    let mut names = Vec::new();
    for line in normalize_line_endings(section_body).split('\n') {
        if let Some(caps) = REQUIREMENT_HEADER.captures(line) {
            names.push(normalize_requirement_name(&caps[1]));
        }
    }
    names
}

fn parse_renamed_pairs(section_body: &str) -> Vec<RenamePair> {
    if section_body.is_empty() {
        return Vec::new();
    }
    let from_re = Regex::new(r"(?i)^\s*-?\s*FROM:\s*`?###\s*Requirement:\s*(.+?)`?\s*$")
        .unwrap_or_else(|err| panic!("invalid FROM rename regex: {err}"));
    let to_re = Regex::new(r"(?i)^\s*-?\s*TO:\s*`?###\s*Requirement:\s*(.+?)`?\s*$")
        .unwrap_or_else(|err| panic!("invalid TO rename regex: {err}"));
    let mut pairs = Vec::new();
    let mut current_from: Option<String> = None;
    for line in normalize_line_endings(section_body).split('\n') {
        if let Some(caps) = from_re.captures(line) {
            current_from = Some(normalize_requirement_name(&caps[1]));
        } else if let Some(caps) = to_re.captures(line) {
            let to = normalize_requirement_name(&caps[1]);
            if let Some(from) = current_from.take() {
                pairs.push(RenamePair { from, to });
            }
        }
    }
    pairs
}

pub fn infer_delta_plan_from_full_spec(
    change_content: &str,
    target_content: &str,
) -> Option<DeltaPlan> {
    let has_delta = [
        r"(?i)^##\s+ADDED Requirements",
        r"(?i)^##\s+MODIFIED Requirements",
        r"(?i)^##\s+REMOVED Requirements",
        r"(?i)^##\s+RENAMED Requirements",
    ]
    .iter()
    .any(|pattern| {
        Regex::new(pattern)
            .unwrap_or_else(|err| panic!("invalid delta section regex {pattern:?}: {err}"))
            .is_match(change_content)
    });
    if has_delta {
        return None;
    }

    let change_parts = extract_requirements_section(change_content);
    if change_parts.body_blocks.is_empty() {
        return None;
    }

    let target_parts = extract_requirements_section(target_content);
    let target_by_name: std::collections::HashMap<_, _> = target_parts
        .body_blocks
        .iter()
        .map(|b| (normalize_requirement_name(&b.name), b))
        .collect();

    let mut added = Vec::new();
    let mut modified = Vec::new();
    for block in &change_parts.body_blocks {
        let key = normalize_requirement_name(&block.name);
        match target_by_name.get(&key) {
            None => added.push(block.clone()),
            Some(existing) if existing.raw.trim() != block.raw.trim() => {
                modified.push(block.clone())
            }
            _ => {}
        }
    }

    if added.is_empty() && modified.is_empty() {
        return None;
    }

    Some(DeltaPlan {
        added,
        modified,
        removed: Vec::new(),
        renamed: Vec::new(),
    })
}

pub fn build_spec_skeleton(capability: &str, change_name: &str) -> String {
    format!(
        "# {capability} capability\n\nCreated by change `{change_name}`. Update purpose as needed.\n\n## Requirements\n"
    )
}
