use super::GEMINI_FREEFORM_INPUT_PROPERTY;
use super::MESSAGES_FREEFORM_INPUT_PROPERTY;
use super::ResponsesApiNamespace;
use super::ResponsesApiWebSearchFilters;
use super::ResponsesApiWebSearchUserLocation;
use super::ToolSpec;
use super::freeform_tool_names;
use crate::AdditionalProperties;
use crate::FreeformTool;
use crate::FreeformToolFormat;
use crate::JsonSchema;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
use crate::create_tools_json_for_gemini_api;
use crate::create_tools_json_for_messages_api;
use crate::create_tools_json_for_responses_api;
use forall_protocol::config_types::WebSearchContextSize;
use forall_protocol::config_types::WebSearchFilters as ConfigWebSearchFilters;
use forall_protocol::config_types::WebSearchUserLocation as ConfigWebSearchUserLocation;
use forall_protocol::config_types::WebSearchUserLocationType;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;

#[test]
fn tool_spec_name_covers_all_variants() {
    assert_eq!(
        ToolSpec::Function(ResponsesApiTool {
            name: "lookup_order".to_string(),
            description: "Look up an order".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })
        .name(),
        "lookup_order"
    );
    assert_eq!(
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: Vec::new(),
        })
        .name(),
        "mcp__demo__"
    );
    assert_eq!(
        ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search for tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None
            ),
        }
        .name(),
        "tool_search"
    );
    assert_eq!(
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        }
        .name(),
        "image_generation"
    );
    assert_eq!(
        ToolSpec::WebSearch {
            external_web_access: Some(true),
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }
        .name(),
        "web_search"
    );
    assert_eq!(
        ToolSpec::Freeform(FreeformTool {
            name: "exec".to_string(),
            description: "Run a command".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: \"exec\"".to_string(),
            },
        })
        .name(),
        "exec"
    );
}

#[test]
fn web_search_config_converts_to_responses_api_types() {
    assert_eq!(
        ResponsesApiWebSearchFilters::from(ConfigWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }
    );
    assert_eq!(
        ResponsesApiWebSearchUserLocation::from(ConfigWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }),
        ResponsesApiWebSearchUserLocation {
            r#type: WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }
    );
}

#[test]
fn create_tools_json_for_responses_api_includes_top_level_name() {
    assert_eq!(
        create_tools_json_for_responses_api(&[ToolSpec::Function(ResponsesApiTool {
            name: "demo".to_string(),
            description: "A demo tool".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::from([("foo".to_string(), JsonSchema::string(/*description*/ None),)]),
                /*required*/ None,
                /*additional_properties*/ None
            ),
            output_schema: None,
        })])
        .expect("serialize tools"),
        vec![json!({
            "type": "function",
            "name": "demo",
            "description": "A demo tool",
            "strict": false,
            "parameters": {
                "type": "object",
                "properties": {
                    "foo": { "type": "string" }
                },
            },
        })]
    );
}

#[test]
fn namespace_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: "lookup_order".to_string(),
                description: "Look up an order".to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([(
                        "order_id".to_string(),
                        JsonSchema::string(/*description*/ None),
                    )]),
                    /*required*/ None,
                    /*additional_properties*/ None,
                ),
                output_schema: None,
            })],
        }))
        .expect("serialize namespace tool"),
        json!({
            "type": "namespace",
            "name": "mcp__demo__",
            "description": "Demo tools",
            "tools": [
                {
                    "type": "function",
                    "name": "lookup_order",
                    "description": "Look up an order",
                    "strict": false,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "order_id": { "type": "string" },
                        },
                    },
                },
            ],
        })
    );
}

#[test]
fn web_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::WebSearch {
            external_web_access: Some(true),
            index_gated_web_access: None,
            filters: Some(ResponsesApiWebSearchFilters {
                allowed_domains: Some(vec!["example.com".to_string()]),
            }),
            user_location: Some(ResponsesApiWebSearchUserLocation {
                r#type: WebSearchUserLocationType::Approximate,
                country: Some("US".to_string()),
                region: Some("California".to_string()),
                city: Some("San Francisco".to_string()),
                timezone: Some("America/Los_Angeles".to_string()),
            }),
            search_context_size: Some(WebSearchContextSize::High),
            search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
        })
        .expect("serialize web_search"),
        json!({
            "type": "web_search",
            "external_web_access": true,
            "filters": {
                "allowed_domains": ["example.com"],
            },
            "user_location": {
                "type": "approximate",
                "country": "US",
                "region": "California",
                "city": "San Francisco",
                "timezone": "America/Los_Angeles",
            },
            "search_context_size": "high",
            "search_content_types": ["text", "image"],
        })
    );
}

#[test]
fn tool_search_tool_spec_serializes_expected_wire_shape() {
    assert_eq!(
        serde_json::to_value(ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search app tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::from([(
                    "query".to_string(),
                    JsonSchema::string(Some("Tool search query".to_string()),),
                )]),
                Some(vec!["query".to_string()]),
                Some(AdditionalProperties::Boolean(false))
            ),
        })
        .expect("serialize tool_search"),
        json!({
            "type": "tool_search",
            "execution": "sync",
            "description": "Search app tools",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Tool search query",
                    }
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        })
    );
}

#[test]
fn create_tools_json_for_messages_api_maps_function_tools() {
    let tools = create_tools_json_for_messages_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("foo".to_string(), JsonSchema::string(/*description*/ None))]),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: None,
    })])
    .expect("map tools");

    assert_eq!(
        serde_json::to_value(&tools).expect("serialize messages tools"),
        json!([
            {
                "name": "demo",
                "description": "A demo tool",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "foo": { "type": "string" }
                    },
                },
            }
        ])
    );
}

#[test]
fn create_tools_json_for_messages_api_skips_unmappable_variants() {
    let tools = create_tools_json_for_messages_api(&[
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: Vec::new(),
        }),
        ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search app tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
        },
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        },
        ToolSpec::WebSearch {
            external_web_access: None,
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        },
        ToolSpec::Function(ResponsesApiTool {
            name: "kept".to_string(),
            // An empty description is omitted rather than sent as "".
            description: String::new(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        }),
    ])
    .expect("map tools");

    assert_eq!(
        serde_json::to_value(&tools).expect("serialize messages tools"),
        json!([
            {
                "name": "kept",
                "input_schema": { "type": "object", "properties": {} },
            }
        ])
    );
}

#[test]
fn create_tools_json_for_messages_api_maps_freeform_tools_to_single_string_parameter() {
    let tools = create_tools_json_for_messages_api(&[ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: "Use the `apply_patch` tool to edit files.".to_string(),
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: \"*** Begin Patch\"".to_string(),
        },
    })])
    .expect("map tools");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "apply_patch");
    assert_eq!(
        tools[0].input_schema,
        json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description":
                        "Raw payload text conforming to the lark grammar in this tool's description.",
                }
            },
            "required": ["input"],
        })
    );

    let description = tools[0]
        .description
        .as_deref()
        .expect("freeform tools always carry a description");
    assert!(
        description.starts_with("Use the `apply_patch` tool to edit files."),
        "tool description should be preserved: {description}"
    );
    assert!(
        description.contains(MESSAGES_FREEFORM_INPUT_PROPERTY),
        "description should name the input parameter: {description}"
    );
    assert!(
        description.contains("start: \"*** Begin Patch\""),
        "grammar definition should be folded into the description: {description}"
    );
}

#[test]
fn freeform_tool_names_lists_only_freeform_tools() {
    let names = freeform_tool_names(&[
        ToolSpec::Function(ResponsesApiTool {
            name: "shell".to_string(),
            description: "Run a command".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        }),
        ToolSpec::Freeform(FreeformTool {
            name: "apply_patch".to_string(),
            description: "Edit files".to_string(),
            format: FreeformToolFormat {
                r#type: "grammar".to_string(),
                syntax: "lark".to_string(),
                definition: "start: patch".to_string(),
            },
        }),
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        },
    ]);

    assert_eq!(names, vec!["apply_patch".to_string()]);
}

#[test]
fn create_tools_json_for_gemini_api_maps_function_tools() {
    let tools = create_tools_json_for_gemini_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "demo".to_string(),
        description: "A demo tool".to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([("foo".to_string(), JsonSchema::string(/*description*/ None))]),
            /*required*/ None,
            /*additional_properties*/ None,
        ),
        output_schema: None,
    })])
    .expect("map tools");

    assert_eq!(
        serde_json::to_value(&tools).expect("serialize gemini tools"),
        json!([
            {
                "name": "demo",
                "description": "A demo tool",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "foo": { "type": "string" }
                    },
                },
            }
        ])
    );
}

#[test]
fn create_tools_json_for_gemini_api_skips_unmappable_variants() {
    let tools = create_tools_json_for_gemini_api(&[
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "mcp__demo__".to_string(),
            description: "Demo tools".to_string(),
            tools: Vec::new(),
        }),
        ToolSpec::ToolSearch {
            execution: "sync".to_string(),
            description: "Search app tools".to_string(),
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
        },
        ToolSpec::ImageGeneration {
            output_format: "png".to_string(),
        },
        ToolSpec::WebSearch {
            external_web_access: None,
            index_gated_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        },
        ToolSpec::Function(ResponsesApiTool {
            name: "kept".to_string(),
            // An empty description is omitted rather than sent as "".
            description: String::new(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                BTreeMap::new(),
                /*required*/ None,
                /*additional_properties*/ None,
            ),
            output_schema: None,
        }),
    ])
    .expect("map tools");

    assert_eq!(
        serde_json::to_value(&tools).expect("serialize gemini tools"),
        json!([
            {
                "name": "kept",
                "parameters": { "type": "object", "properties": {} },
            }
        ])
    );
}

#[test]
fn create_tools_json_for_gemini_api_maps_freeform_tools_to_single_string_parameter() {
    let tools = create_tools_json_for_gemini_api(&[ToolSpec::Freeform(FreeformTool {
        name: "apply_patch".to_string(),
        description: "Use the `apply_patch` tool to edit files.".to_string(),
        format: FreeformToolFormat {
            r#type: "grammar".to_string(),
            syntax: "lark".to_string(),
            definition: "start: \"*** Begin Patch\"".to_string(),
        },
    })])
    .expect("map tools");

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "apply_patch");
    assert_eq!(
        tools[0].parameters,
        Some(json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description":
                        "Raw payload text conforming to the lark grammar in this tool's description.",
                }
            },
            "required": ["input"],
        }))
    );

    let description = tools[0]
        .description
        .as_deref()
        .expect("freeform tools always carry a description");
    assert!(
        description.starts_with("Use the `apply_patch` tool to edit files."),
        "tool description should be preserved: {description}"
    );
    assert!(
        description.contains(GEMINI_FREEFORM_INPUT_PROPERTY),
        "description should name the input parameter: {description}"
    );
    assert!(
        description.contains("start: \"*** Begin Patch\""),
        "grammar definition should be folded into the description: {description}"
    );
}

/// Gemini validates `parameters` against an OpenAPI 3.0 subset and rejects
/// unknown keywords with `INVALID_ARGUMENT`, so JSON-Schema-only constructs must
/// be stripped rather than forwarded.
#[test]
fn create_tools_json_for_gemini_api_strips_unsupported_schema_keywords() {
    let tools = create_tools_json_for_gemini_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "strict".to_string(),
        description: "d".to_string(),
        strict: true,
        defer_loading: None,
        parameters: JsonSchema::object(
            BTreeMap::from([(
                "nested".to_string(),
                JsonSchema::object(
                    BTreeMap::from([(
                        "leaf".to_string(),
                        JsonSchema::string(Some("a leaf".to_string())),
                    )]),
                    Some(vec!["leaf".to_string()]),
                    Some(AdditionalProperties::Boolean(false)),
                ),
            )]),
            Some(vec!["nested".to_string()]),
            Some(AdditionalProperties::Boolean(false)),
        ),
        output_schema: None,
    })])
    .expect("map tools");

    assert_eq!(
        tools[0].parameters,
        Some(json!({
            "type": "object",
            "properties": {
                "nested": {
                    "type": "object",
                    "properties": {
                        "leaf": {"type": "string", "description": "a leaf"}
                    },
                    "required": ["leaf"],
                }
            },
            "required": ["nested"],
        })),
        "additionalProperties must be stripped at every level"
    );
}

#[test]
fn create_tools_json_for_gemini_api_collapses_nullable_type_unions() {
    let tools = create_tools_json_for_gemini_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "nullable".to_string(),
        description: "d".to_string(),
        strict: false,
        defer_loading: None,
        parameters: serde_json::from_value(json!({
            "type": "object",
            "properties": {
                "maybe": {"type": ["string", "null"]}
            }
        }))
        .expect("parse schema"),
        output_schema: None,
    })])
    .expect("map tools");

    assert_eq!(
        tools[0].parameters,
        Some(json!({
            "type": "object",
            "properties": {
                "maybe": {"type": "string", "nullable": true}
            },
        }))
    );
}

/// `$ref`/`$defs` have no OpenAPI-subset equivalent; a schema left with nothing
/// Gemini understands must omit `parameters` rather than send `{}`.
#[test]
fn create_tools_json_for_gemini_api_omits_parameters_with_no_supported_keywords() {
    let tools = create_tools_json_for_gemini_api(&[ToolSpec::Function(ResponsesApiTool {
        name: "ref_only".to_string(),
        description: "d".to_string(),
        strict: false,
        defer_loading: None,
        parameters: serde_json::from_value(json!({"$ref": "#/$defs/Thing"})).expect("parse schema"),
        output_schema: None,
    })])
    .expect("map tools");

    assert_eq!(tools[0].parameters, None);
}
