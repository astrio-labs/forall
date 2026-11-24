use forall_tools::JsonSchema;
use forall_tools::ResponsesApiNamespace;
use forall_tools::ResponsesApiNamespaceTool;
use forall_tools::ResponsesApiTool;
use forall_tools::ToolSpec;
use std::collections::BTreeMap;

pub const SPECS_NAMESPACE: &str = "specs";
const NAMESPACE_DESCRIPTION: &str =
    "Forall spec workflow and verification — propose changes, track artifacts, run checks.";

fn namespace_tool(
    name: &str,
    description: &str,
    properties: BTreeMap<String, JsonSchema>,
    required: Vec<String>,
) -> ToolSpec {
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: SPECS_NAMESPACE.to_string(),
        description: NAMESPACE_DESCRIPTION.to_string(),
        tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
            name: name.to_string(),
            description: description.to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(properties, Some(required), Some(false.into())),
            output_schema: None,
        })],
    })
}

pub fn create_list_tool() -> ToolSpec {
    namespace_tool(
        "list",
        "List active Forall spec changes in the current project.",
        BTreeMap::new(),
        Vec::new(),
    )
}

pub fn create_propose_tool() -> ToolSpec {
    namespace_tool(
        "propose",
        "Start a new Forall change (creates proposal + mapping delta). Required before editing application code.",
        BTreeMap::from([(
            "name".to_string(),
            JsonSchema::string(Some("Change name in kebab-case.".to_string())),
        )]),
        vec!["name".to_string()],
    )
}

pub fn create_status_tool() -> ToolSpec {
    namespace_tool(
        "status",
        "Show artifact workflow progress for a change (ready / blocked / done).",
        BTreeMap::from([(
            "change".to_string(),
            JsonSchema::string(Some("Active change name.".to_string())),
        )]),
        vec!["change".to_string()],
    )
}

pub fn create_instructions_tool() -> ToolSpec {
    namespace_tool(
        "instructions",
        "Get template and instructions for the next workflow artifact.",
        BTreeMap::from([
            (
                "change".to_string(),
                JsonSchema::string(Some("Change name.".to_string())),
            ),
            (
                "artifact".to_string(),
                JsonSchema::string(Some(
                    "Artifact id: proposal, specs, design, verification, tasks, or apply."
                        .to_string(),
                )),
            ),
        ]),
        vec!["change".to_string(), "artifact".to_string()],
    )
}

pub fn create_check_tool() -> ToolSpec {
    namespace_tool(
        "check",
        "Run the Forall verification gate (mapping, proofs, intent, scenarios, property-tests).",
        BTreeMap::from([
            (
                "change".to_string(),
                JsonSchema::string(Some("Optional: scope to one change.".to_string())),
            ),
            (
                "strict".to_string(),
                JsonSchema::boolean(Some("Fail on warnings when true.".to_string())),
            ),
            (
                "pbt_seed".to_string(),
                JsonSchema::integer(Some("Reproducible PBT seed (default 42).".to_string())),
            ),
            (
                "pbt_examples".to_string(),
                JsonSchema::integer(Some(
                    "Number of PBT examples per property (default 100).".to_string(),
                )),
            ),
        ]),
        Vec::new(),
    )
}

pub fn create_sync_tool() -> ToolSpec {
    namespace_tool(
        "sync",
        "Merge delta specs and stub //@ annotations for a change.",
        BTreeMap::from([(
            "change".to_string(),
            JsonSchema::string(Some("Change name.".to_string())),
        )]),
        vec!["change".to_string()],
    )
}

pub fn create_archive_tool() -> ToolSpec {
    namespace_tool(
        "archive",
        "Verify, merge specs + mapping, and archive a completed change.",
        BTreeMap::from([(
            "change".to_string(),
            JsonSchema::string(Some("Change name.".to_string())),
        )]),
        vec!["change".to_string()],
    )
}
