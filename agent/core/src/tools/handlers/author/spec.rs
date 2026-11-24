use forall_tools::JsonSchema;
use forall_tools::ResponsesApiNamespace;
use forall_tools::ResponsesApiNamespaceTool;
use forall_tools::ResponsesApiTool;
use forall_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const AUTHOR_NAMESPACE: &str = "author";
const NAMESPACE_DESCRIPTION: &str =
    "Local Forall project authoring — inspect, initialize, discover, scaffold, and validate.";

fn namespace_tool(
    name: &str,
    description: &str,
    properties: BTreeMap<String, JsonSchema>,
    required: Vec<String>,
) -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: AUTHOR_NAMESPACE.to_string(),
        description: NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: name.to_string(),
            description: description.to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "ok": { "type": "boolean" },
                    "result": { "type": "object", "additionalProperties": true },
                    "error": { "type": "object", "additionalProperties": true }
                },
                "required": ["ok"],
                "additionalProperties": false
            })),
        })],
    })
}

fn mode_schema() -> JsonSchema {
    JsonSchema::string_enum(
        vec![json!("preview"), json!("apply")],
        Some("Preview computes changes without writing; apply writes them.".to_string()),
    )
}

fn optional_string(description: &str) -> JsonSchema {
    JsonSchema::string(Some(description.to_string()))
}

fn string_array(description: &str) -> JsonSchema {
    JsonSchema::array(
        JsonSchema::string(/*description*/ None),
        Some(description.to_string()),
    )
}

pub fn create_status_tool() -> ToolSpec {
    namespace_tool(
        "status",
        "Inspect the local Forall project.",
        BTreeMap::new(),
        Vec::new(),
    )
}

pub fn create_init_tool() -> ToolSpec {
    namespace_tool(
        "init",
        "Preview or initialize the local Forall project layout.",
        BTreeMap::from([("mode".to_string(), mode_schema())]),
        vec!["mode".to_string()],
    )
}

pub fn create_discover_tool() -> ToolSpec {
    namespace_tool(
        "discover",
        "Discover candidate TypeScript, Rust, and Java symbols without writing files.",
        BTreeMap::from([
            (
                "files".to_string(),
                string_array("Optional project-relative source files to inspect."),
            ),
            (
                "languages".to_string(),
                JsonSchema::array(
                    JsonSchema::string_enum(
                        vec![
                            json!("type_script"),
                            json!("rust"),
                            json!("java"),
                            json!("c"),
                        ],
                        None,
                    ),
                    Some("Languages to scan when files are omitted.".to_string()),
                ),
            ),
        ]),
        Vec::new(),
    )
}

fn code_ref_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "file".to_string(),
                optional_string("Project-relative source file."),
            ),
            (
                "symbols".to_string(),
                string_array("Mapped source symbols."),
            ),
        ]),
        Some(vec!["file".to_string(), "symbols".to_string()]),
        Some(false.into()),
    )
}

fn requirement_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            ("id".to_string(), optional_string("Stable requirement id.")),
            (
                "capability".to_string(),
                optional_string("Requirement capability."),
            ),
            (
                "requirement".to_string(),
                optional_string("Human-readable requirement."),
            ),
            (
                "verified".to_string(),
                JsonSchema::boolean(Some("Whether the requirement is proved.".to_string())),
            ),
            (
                "property_tested".to_string(),
                JsonSchema::boolean(Some("Whether a property test covers it.".to_string())),
            ),
            ("code".to_string(), code_ref_schema()),
            (
                "contract".to_string(),
                optional_string("Optional contract text."),
            ),
        ]),
        Some(vec![
            "id".to_string(),
            "capability".to_string(),
            "requirement".to_string(),
        ]),
        Some(false.into()),
    )
}

pub fn create_upsert_requirements_tool() -> ToolSpec {
    namespace_tool(
        "upsert_requirements",
        "Preview or merge explicit requirements into the project mapping.",
        BTreeMap::from([
            ("mode".to_string(), mode_schema()),
            (
                "expected_mapping_sha256".to_string(),
                optional_string("Expected mapping SHA-256 for optimistic concurrency."),
            ),
            (
                "requirements".to_string(),
                JsonSchema::array(
                    requirement_schema(),
                    Some("Requirements to merge.".to_string()),
                ),
            ),
        ]),
        vec!["mode".to_string(), "requirements".to_string()],
    )
}

fn contract_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "file".to_string(),
                optional_string("Project-relative source file."),
            ),
            (
                "symbol".to_string(),
                optional_string("Exact discovery selector."),
            ),
            (
                "requires".to_string(),
                string_array("Precondition clauses."),
            ),
            (
                "ensures".to_string(),
                string_array("Postcondition clauses."),
            ),
            (
                "expected_sha256".to_string(),
                optional_string("Expected source SHA-256 for optimistic concurrency."),
            ),
        ]),
        Some(vec!["file".to_string(), "symbol".to_string()]),
        Some(false.into()),
    )
}

pub fn create_scaffold_contracts_tool() -> ToolSpec {
    namespace_tool(
        "scaffold_contracts",
        "Preview or insert caller-supplied Forall contracts at mapped symbols.",
        BTreeMap::from([
            ("mode".to_string(), mode_schema()),
            (
                "contracts".to_string(),
                JsonSchema::array(contract_schema(), Some("Contract targets.".to_string())),
            ),
        ]),
        vec!["mode".to_string(), "contracts".to_string()],
    )
}

pub fn create_validate_tool() -> ToolSpec {
    namespace_tool(
        "validate",
        "Validate local mapping, paths, and mapped symbols without running verification.",
        BTreeMap::new(),
        Vec::new(),
    )
}

pub fn create_scaffold_property_tool() -> ToolSpec {
    namespace_tool(
        "scaffold_property",
        "Preview or create a caller-supplied property test and mapping link.",
        BTreeMap::from([
            ("mode".to_string(), mode_schema()),
            (
                "requirement_id".to_string(),
                optional_string("Requirement receiving the property link."),
            ),
            (
                "body".to_string(),
                optional_string("Complete TypeScript property module body."),
            ),
            (
                "symbol".to_string(),
                optional_string("Optional exported property symbol."),
            ),
            (
                "expected_sha256".to_string(),
                optional_string("Expected property-file SHA-256 when replacing it."),
            ),
            (
                "expected_mapping_sha256".to_string(),
                optional_string("Expected mapping SHA-256 for optimistic concurrency."),
            ),
        ]),
        vec![
            "mode".to_string(),
            "requirement_id".to_string(),
            "body".to_string(),
        ],
    )
}

#[cfg(test)]
#[path = "spec_tests.rs"]
mod tests;
