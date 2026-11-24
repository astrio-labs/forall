use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiTool;
use forall_protocol::config_types::WebSearchContextSize;
use forall_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use forall_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use forall_protocol::config_types::WebSearchUserLocationType;
use serde::Serialize;
use serde_json::Value;
use tracing::warn;

/// When serialized as JSON, this produces a valid "Tool" in the OpenAI
/// Responses API.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(ResponsesApiNamespace),
    #[serde(rename = "tool_search")]
    ToolSearch {
        execution: String,
        description: String,
        parameters: JsonSchema,
    },
    #[serde(rename = "image_generation")]
    ImageGeneration { output_format: String },
    // TODO: Understand why we get an error on web_search although the API docs
    // say it's supported.
    // https://platform.openai.com/docs/guides/tools-web-search?api-mode=responses#:~:text=%7B%20type%3A%20%22web_search%22%20%7D%2C
    // The `external_web_access` field determines whether the web search is over
    // cached or live content.
    // https://platform.openai.com/docs/guides/tools-web-search#live-internet-access
    #[serde(rename = "web_search")]
    WebSearch {
        #[serde(skip_serializing_if = "Option::is_none")]
        external_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        index_gated_web_access: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filters: Option<ResponsesApiWebSearchFilters>,
        #[serde(skip_serializing_if = "Option::is_none")]
        user_location: Option<ResponsesApiWebSearchUserLocation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_context_size: Option<WebSearchContextSize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        search_content_types: Option<Vec<String>>,
    },
    #[serde(rename = "custom")]
    Freeform(FreeformTool),
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        match self {
            ToolSpec::Function(tool) => tool.name.as_str(),
            ToolSpec::Namespace(namespace) => namespace.name.as_str(),
            ToolSpec::ToolSearch { .. } => "tool_search",
            ToolSpec::ImageGeneration { .. } => "image_generation",
            ToolSpec::WebSearch { .. } => "web_search",
            ToolSpec::Freeform(tool) => tool.name.as_str(),
        }
    }
}

impl From<LoadableToolSpec> for ToolSpec {
    fn from(value: LoadableToolSpec) -> Self {
        match value {
            LoadableToolSpec::Function(tool) => ToolSpec::Function(tool),
            LoadableToolSpec::Namespace(namespace) => ToolSpec::Namespace(namespace),
        }
    }
}

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
pub fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();

    for tool in tools {
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}

/// A tool definition in the Anthropic Messages API shape.
///
/// Serializes as `{"name", "description", "input_schema"}`, the entry shape of
/// the `tools` array in `POST /v1/messages`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MessagesApiToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

/// Name of the single string property that carries a freeform tool's payload
/// on the Messages wire.
///
/// The Messages API has no grammar-constrained ("custom") tool type: every
/// entry in `tools` must declare a JSON `input_schema`. Freeform tools are
/// therefore advertised as a one-parameter object tool whose grammar lives in
/// the description, and the SSE adapter unwraps this property back into
/// `ResponseItem::CustomToolCall::input` so the freeform handlers still see raw
/// payload text. `forall-api` keeps its own copy of this name (it does not
/// depend on this crate); the two must stay in sync.
pub const MESSAGES_FREEFORM_INPUT_PROPERTY: &str = "input";

/// Maps tool specs to the Anthropic Messages API `tools` array shape.
///
/// `Function` tools map directly. `Freeform` tools have no native analogue and
/// are re-shaped into a single-string-parameter tool (see
/// [`MESSAGES_FREEFORM_INPUT_PROPERTY`]) rather than dropped — the bundled
/// Claude catalog advertises `apply_patch` as a freeform tool, so dropping it
/// would silently strip repository editing from every Anthropic coding turn.
///
/// Namespace, tool search, image generation, and web search remain OpenAI
/// Responses concepts with no Anthropic analogue and are skipped with a
/// warning. Provider capabilities are expected to disable those tools before a
/// prompt is built; this skip is defense in depth for anything that slips
/// through.
pub fn create_tools_json_for_messages_api(
    tools: &[ToolSpec],
) -> Result<Vec<MessagesApiToolSpec>, serde_json::Error> {
    let mut messages_tools = Vec::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(function_tool) => messages_tools.push(MessagesApiToolSpec {
                name: function_tool.name.clone(),
                description: (!function_tool.description.is_empty())
                    .then(|| function_tool.description.clone()),
                input_schema: serde_json::to_value(&function_tool.parameters)?,
            }),
            ToolSpec::Freeform(freeform_tool) => {
                messages_tools.push(freeform_tool_for_messages_api(freeform_tool));
            }
            ToolSpec::Namespace(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. } => {
                warn!(
                    "skipping tool `{}`: it has no Messages API representation",
                    tool.name()
                );
            }
        }
    }

    Ok(messages_tools)
}

/// Re-shapes a freeform tool into the Messages `tools` entry shape.
///
/// The grammar is folded into the description because that is the only place
/// the Messages API can carry it; without it the model has no way to know what
/// the opaque `input` string should contain.
fn freeform_tool_for_messages_api(tool: &FreeformTool) -> MessagesApiToolSpec {
    let FreeformTool {
        name,
        description,
        format:
            FreeformToolFormat {
                r#type,
                syntax,
                definition,
            },
    } = tool;

    let description = format!(
        "{description}\n\n\
         Call this tool with a single parameter, `{MESSAGES_FREEFORM_INPUT_PROPERTY}`, whose value \
         is the raw payload as one JSON string. Do not restructure the payload into JSON fields — \
         only the surrounding JSON string quoting/escaping is added. The payload must conform to \
         the following {syntax} {type}:\n\n{definition}"
    );
    let input_description =
        format!("Raw payload text conforming to the {syntax} {type} in this tool's description.");

    MessagesApiToolSpec {
        name: name.clone(),
        description: Some(description),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                (MESSAGES_FREEFORM_INPUT_PROPERTY): {
                    "type": "string",
                    "description": input_description,
                }
            },
            "required": [MESSAGES_FREEFORM_INPUT_PROPERTY],
        }),
    }
}

/// Names of the freeform tools in `tools`.
///
/// The Messages SSE adapter needs this set to decide which `tool_use` blocks to
/// re-emit as `CustomToolCall`s; nothing on the wire distinguishes a re-shaped
/// freeform tool from an ordinary function tool.
pub fn freeform_tool_names(tools: &[ToolSpec]) -> Vec<String> {
    tools
        .iter()
        .filter_map(|tool| match tool {
            ToolSpec::Freeform(freeform_tool) => Some(freeform_tool.name.clone()),
            ToolSpec::Function(_)
            | ToolSpec::Namespace(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. } => None,
        })
        .collect()
}

/// A tool definition in the native Gemini API shape.
///
/// Serializes as `{"name", "description", "parameters"}`, the entry shape of
/// `tools[].functionDeclarations` in `models/{model}:streamGenerateContent`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GeminiApiToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// Name of the single string property that carries a freeform tool's payload on
/// the Gemini wire.
///
/// Gemini has no grammar-constrained ("custom") tool type: every function
/// declaration must declare an OpenAPI `parameters` schema. Freeform tools are
/// therefore advertised as a one-parameter object tool whose grammar lives in
/// the description, and the SSE adapter unwraps this property back into
/// `ResponseItem::CustomToolCall::input` so the freeform handlers still see raw
/// payload text. `forall-api` keeps its own copy of this name (it does not
/// depend on this crate); the two must stay in sync.
pub const GEMINI_FREEFORM_INPUT_PROPERTY: &str = "input";

/// Maps tool specs to the Gemini `functionDeclarations` array shape.
///
/// `Function` tools map directly, with their schema narrowed to the OpenAPI
/// subset Gemini accepts (see [`gemini_parameter_schema`]). `Freeform` tools
/// have no native analogue and are re-shaped into a single-string-parameter
/// declaration (see [`GEMINI_FREEFORM_INPUT_PROPERTY`]) rather than dropped —
/// the bundled Gemini catalog advertises `apply_patch` as a freeform tool, so
/// dropping it would silently strip repository editing from every Gemini coding
/// turn.
///
/// Namespace, tool search, image generation, and web search remain OpenAI
/// Responses concepts with no Gemini analogue and are skipped with a warning.
/// Provider capabilities are expected to disable those tools before a prompt is
/// built; this skip is defense in depth for anything that slips through.
pub fn create_tools_json_for_gemini_api(
    tools: &[ToolSpec],
) -> Result<Vec<GeminiApiToolSpec>, serde_json::Error> {
    let mut gemini_tools = Vec::new();

    for tool in tools {
        match tool {
            ToolSpec::Function(function_tool) => gemini_tools.push(GeminiApiToolSpec {
                name: function_tool.name.clone(),
                description: (!function_tool.description.is_empty())
                    .then(|| function_tool.description.clone()),
                parameters: gemini_parameter_schema(&serde_json::to_value(
                    &function_tool.parameters,
                )?),
            }),
            ToolSpec::Freeform(freeform_tool) => {
                gemini_tools.push(freeform_tool_for_gemini_api(freeform_tool));
            }
            ToolSpec::Namespace(_)
            | ToolSpec::ToolSearch { .. }
            | ToolSpec::ImageGeneration { .. }
            | ToolSpec::WebSearch { .. } => {
                warn!(
                    "skipping tool `{}`: it has no Gemini API representation",
                    tool.name()
                );
            }
        }
    }

    Ok(gemini_tools)
}

/// Re-shapes a freeform tool into a Gemini function declaration.
///
/// The grammar is folded into the description because that is the only place a
/// function declaration can carry it; without it the model has no way to know
/// what the opaque `input` string should contain.
fn freeform_tool_for_gemini_api(tool: &FreeformTool) -> GeminiApiToolSpec {
    let FreeformTool {
        name,
        description,
        format:
            FreeformToolFormat {
                r#type,
                syntax,
                definition,
            },
    } = tool;

    let description = format!(
        "{description}\n\n\
         Call this tool with a single parameter, `{GEMINI_FREEFORM_INPUT_PROPERTY}`, whose value \
         is the raw payload as one JSON string. Do not restructure the payload into JSON fields — \
         only the surrounding JSON string quoting/escaping is added. The payload must conform to \
         the following {syntax} {type}:\n\n{definition}"
    );
    let input_description =
        format!("Raw payload text conforming to the {syntax} {type} in this tool's description.");

    GeminiApiToolSpec {
        name: name.clone(),
        description: Some(description),
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                (GEMINI_FREEFORM_INPUT_PROPERTY): {
                    "type": "string",
                    "description": input_description,
                }
            },
            "required": [GEMINI_FREEFORM_INPUT_PROPERTY],
        })),
    }
}

/// Keys of the OpenAPI 3.0 `Schema` subset Gemini accepts in
/// `functionDeclarations[].parameters`.
///
/// Gemini validates this schema strictly and rejects unknown fields with
/// `INVALID_ARGUMENT`, so anything outside this list must be stripped rather
/// than forwarded. Notably absent — and present in our tool schemas — are
/// `$ref`, `$defs`, `definitions`, `additionalProperties`, `oneOf`, `allOf`, and
/// the Responses-only `encrypted` marker.
const GEMINI_SCHEMA_KEYS: [&str; 15] = [
    "type",
    "format",
    "title",
    "description",
    "nullable",
    "enum",
    "items",
    "properties",
    "required",
    "anyOf",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
    "pattern",
];

/// Narrows a JSON Schema to the OpenAPI subset Gemini accepts.
///
/// Returns `None` for a schema that carries nothing Gemini understands, so the
/// caller omits `parameters` entirely (a parameterless tool) instead of sending
/// an empty object.
///
/// This is lossy by necessity and the loss is one-directional: dropping
/// `additionalProperties` or an `allOf` branch widens what the model may send,
/// and the tool handler still validates its own input. `$ref`/`$defs` are
/// dropped rather than inlined — a `$ref`-only subschema becomes an
/// unconstrained value, which the model fills from the surrounding
/// `description`.
fn gemini_parameter_schema(schema: &Value) -> Option<Value> {
    let sanitized = sanitize_gemini_schema(schema);
    match &sanitized {
        Value::Object(map) if map.is_empty() => None,
        _ => Some(sanitized),
    }
}

fn sanitize_gemini_schema(schema: &Value) -> Value {
    let Value::Object(map) = schema else {
        // A non-object schema (e.g. `true`) has no OpenAPI equivalent.
        return Value::Object(serde_json::Map::new());
    };

    let mut out = serde_json::Map::new();
    for (key, value) in map {
        if !GEMINI_SCHEMA_KEYS.contains(&key.as_str()) {
            continue;
        }
        match key.as_str() {
            "type" => {
                let (schema_type, nullable) = gemini_schema_type(value);
                if let Some(schema_type) = schema_type {
                    out.insert("type".to_string(), Value::String(schema_type));
                }
                if nullable {
                    out.insert("nullable".to_string(), Value::Bool(true));
                }
            }
            "items" => {
                out.insert("items".to_string(), sanitize_gemini_schema(value));
            }
            "properties" => {
                if let Value::Object(properties) = value {
                    let sanitized: serde_json::Map<String, Value> = properties
                        .iter()
                        .map(|(name, property)| (name.clone(), sanitize_gemini_schema(property)))
                        .collect();
                    out.insert("properties".to_string(), Value::Object(sanitized));
                }
            }
            "anyOf" => {
                if let Value::Array(variants) = value {
                    let sanitized: Vec<Value> =
                        variants.iter().map(sanitize_gemini_schema).collect();
                    out.insert("anyOf".to_string(), Value::Array(sanitized));
                }
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(out)
}

/// Converts a JSON Schema `type` into Gemini's single-valued `type` plus a
/// `nullable` flag.
///
/// JSON Schema expresses "string or null" as a type union, which Gemini's
/// `Schema.type` cannot hold; the union is collapsed to its first non-null
/// member with `nullable: true`.
fn gemini_schema_type(value: &Value) -> (Option<String>, bool) {
    match value {
        Value::String(name) => (Some(name.clone()), false),
        Value::Array(names) => {
            let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
            let nullable = names.contains(&"null");
            let primary = names
                .into_iter()
                .find(|name| *name != "null")
                .map(str::to_string);
            (primary, nullable)
        }
        _ => (None, false),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_domains: Option<Vec<String>>,
}

impl From<ConfigWebSearchFilters> for ResponsesApiWebSearchFilters {
    fn from(filters: ConfigWebSearchFilters) -> Self {
        Self {
            allowed_domains: filters.allowed_domains,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResponsesApiWebSearchUserLocation {
    #[serde(rename = "type")]
    pub r#type: WebSearchUserLocationType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl From<ConfigWebSearchUserLocation> for ResponsesApiWebSearchUserLocation {
    fn from(user_location: ConfigWebSearchUserLocation) -> Self {
        Self {
            r#type: user_location.r#type,
            country: user_location.country,
            region: user_location.region,
            city: user_location.city,
            timezone: user_location.timezone,
        }
    }
}

#[cfg(test)]
#[path = "tool_spec_tests.rs"]
mod tests;
