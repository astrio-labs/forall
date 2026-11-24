use forall_tools::JsonSchema;
use forall_tools::ResponsesApiNamespace;
use forall_tools::ResponsesApiNamespaceTool;
use forall_tools::ResponsesApiTool;
use forall_tools::ToolSpec;
use serde_json::json;
use std::collections::BTreeMap;

pub const VERIFY_NAMESPACE: &str = "verify";
const NAMESPACE_DESCRIPTION: &str = "Hosted Forall verification — safely submit workspace snapshots, inspect jobs, cancel work, and explain findings.";

fn namespace_tool(
    name: &str,
    description: &str,
    properties: BTreeMap<String, JsonSchema>,
    required: Vec<String>,
) -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: VERIFY_NAMESPACE.to_string(),
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

fn string(description: &str) -> JsonSchema {
    JsonSchema::string(Some(description.to_string()))
}

fn string_array(description: &str) -> JsonSchema {
    JsonSchema::array(JsonSchema::string(None), Some(description.to_string()))
}

fn source_schema() -> JsonSchema {
    JsonSchema::object(
        BTreeMap::from([
            (
                "type".to_string(),
                JsonSchema::string_enum(
                    vec![json!("workspace"), json!("github")],
                    Some("Source kind. Defaults to the current workspace.".to_string()),
                ),
            ),
            (
                "paths".to_string(),
                string_array(
                    "Optional workspace-relative files. When omitted, Forall packages project verification inputs and mapped sources.",
                ),
            ),
            (
                "repository".to_string(),
                string("Public GitHub owner/repository."),
            ),
            (
                "ref".to_string(),
                string("Git ref. Defaults to main for GitHub sources."),
            ),
            (
                "subdirectory".to_string(),
                string("Optional repository-relative subdirectory."),
            ),
        ]),
        None,
        Some(false.into()),
    )
}

pub fn create_submit_tool() -> ToolSpec {
    namespace_tool(
        "submit",
        "Package a confined, secret-filtered source snapshot and submit asynchronous hosted verification.",
        BTreeMap::from([
            ("source".to_string(), source_schema()),
            (
                "change".to_string(),
                string("Optional active change name; omission verifies the project."),
            ),
            (
                "strict".to_string(),
                JsonSchema::boolean(Some("Treat warnings as failures.".to_string())),
            ),
            (
                "phases".to_string(),
                JsonSchema::array(
                    JsonSchema::string_enum(
                        vec![
                            json!("structure"),
                            json!("mapping"),
                            json!("proofs"),
                            json!("intent"),
                            json!("scenarios"),
                            json!("property-tests"),
                            json!("scenario-tests"),
                        ],
                        None,
                    ),
                    Some("Optional verification phases.".to_string()),
                ),
            ),
            (
                "pbt_seed".to_string(),
                JsonSchema::integer(Some("Property-test seed (default 42).".to_string())),
            ),
            (
                "pbt_examples".to_string(),
                JsonSchema::integer(Some(
                    "Property-test example count (default 100).".to_string(),
                )),
            ),
        ]),
        Vec::new(),
    )
}

pub fn create_status_tool() -> ToolSpec {
    namespace_tool(
        "status",
        "Get progress and sanitized results for a hosted verification job.",
        BTreeMap::from([("job_id".to_string(), string("Hosted verification job id."))]),
        vec!["job_id".to_string()],
    )
}

pub fn create_cancel_tool() -> ToolSpec {
    namespace_tool(
        "cancel",
        "Cancel a queued or running hosted verification job.",
        BTreeMap::from([("job_id".to_string(), string("Hosted verification job id."))]),
        vec!["job_id".to_string()],
    )
}

pub fn create_explain_tool() -> ToolSpec {
    namespace_tool(
        "explain",
        "Explain selected findings from a completed hosted verification report.",
        BTreeMap::from([
            ("job_id".to_string(), string("Hosted verification job id.")),
            (
                "issue_indexes".to_string(),
                JsonSchema::array(
                    JsonSchema::integer(None),
                    Some("Optional zero-based issue indexes.".to_string()),
                ),
            ),
            (
                "audience".to_string(),
                JsonSchema::string_enum(
                    vec![json!("concise"), json!("detailed")],
                    Some("Explanation detail level. Defaults to concise.".to_string()),
                ),
            ),
        ]),
        vec!["job_id".to_string()],
    )
}
