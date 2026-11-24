//! Request-side types for the native Google Gemini `generateContent` API.
//!
//! These structs serialize directly into the
//! `POST /v1beta/models/{model}:streamGenerateContent` JSON body — like
//! [`crate::common::ResponsesApiRequest`] and
//! [`crate::messages_common::MessagesApiRequest`], the `Serialize` impls *are*
//! the wire format, with no intermediate DTO.
//!
//! Gemini's JSON is camelCase (it is the proto3 JSON mapping of the
//! `GenerateContentRequest` message), so every field is renamed accordingly.
//! The model slug is NOT a body field: it lives in the request path.

use serde::Serialize;
use serde_json::Value;

/// JSON body for a streaming `models/{model}:streamGenerateContent` request.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiRequest {
    /// Conversation turns. Must be non-empty; roles are `user`/`model`.
    pub contents: Vec<GeminiContent>,
    /// System prompt. A `Content` with no `role`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiSystemInstruction>,
    /// Tool declarations. Gemini nests function tools one level deeper than
    /// other APIs: `tools: [{ functionDeclarations: [...] }]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<GeminiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<GeminiToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
}

/// The `systemInstruction` field: a role-less `Content`.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GeminiSystemInstruction {
    pub parts: Vec<GeminiPart>,
}

/// A single conversation turn in the `contents` array.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GeminiContent {
    pub role: GeminiRole,
    pub parts: Vec<GeminiPart>,
}

/// Gemini names the assistant role `model`, not `assistant`.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GeminiRole {
    User,
    Model,
}

/// Content parts accepted (and emitted) by the Gemini API.
///
/// Unlike the Anthropic Messages API, a Gemini `Part` is not a tagged union: it
/// is a message with mutually exclusive optional fields, so each variant here
/// serializes as an untagged single-key object (`{"text": ...}`,
/// `{"functionCall": {...}}`, …).
///
/// `thoughtSignature` is an opaque token Gemini attaches to reasoning (and
/// function-call) parts so a later turn can continue the same chain of thought.
/// It is carried alongside `text` on the `Thought` variant; see the
/// round-tripping caveat on [`GeminiPart::Thought`].
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum GeminiPart {
    /// Ordinary visible text.
    Text { text: String },
    /// A reasoning ("thought") part. Only returned when
    /// `generationConfig.thinkingConfig.includeThoughts` is set, and only
    /// accepted on replay when it carries the original `thoughtSignature`.
    ///
    /// Replaying a thought part *without* its signature is what Gemini treats
    /// as a fidelity loss (reasoning continuity is dropped) rather than an
    /// error, so the signature is optional here instead of required.
    #[serde(rename_all = "camelCase")]
    Thought {
        text: String,
        thought: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },
    /// Base64 image (or other blob) data.
    #[serde(rename_all = "camelCase")]
    InlineData { inline_data: GeminiBlob },
    /// A tool call. `args` is a JSON *object*, unlike the Responses API's
    /// JSON-encoded `arguments` string.
    #[serde(rename_all = "camelCase")]
    FunctionCall { function_call: GeminiFunctionCall },
    /// A tool result. Gemini matches results to calls by function `name`, not
    /// by an opaque call id.
    #[serde(rename_all = "camelCase")]
    FunctionResponse {
        function_response: GeminiFunctionResponse,
    },
}

/// `inlineData` payload: base64 bytes plus their media type.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiBlob {
    pub mime_type: String,
    pub data: String,
}

/// `functionCall` payload.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GeminiFunctionCall {
    pub name: String,
    /// Always a JSON object on the wire (`{}` when the call takes no
    /// arguments).
    pub args: Value,
}

/// `functionResponse` payload.
///
/// `response` must be a JSON object; scalar tool output is wrapped by the
/// request builder.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: Value,
}

/// One entry of the `tools` array. Gemini groups all function declarations
/// under a single tool entry.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeminiTool {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub function_declarations: Vec<GeminiFunctionDeclaration>,
}

/// A single function tool: `{name, description, parameters}`.
///
/// `parameters` is an OpenAPI 3.0 `Schema` subset, NOT arbitrary JSON Schema —
/// unsupported keywords are rejected with `INVALID_ARGUMENT`, so the request
/// builder sanitizes schemas before they reach this struct.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct GeminiFunctionDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

/// The `toolConfig` request parameter.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiToolConfig {
    pub function_calling_config: GeminiFunctionCallingConfig,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GeminiFunctionCallingConfig {
    pub mode: GeminiFunctionCallingMode,
}

/// Gemini has no parallel-tool-call toggle; `Auto` lets the model return any
/// number of `functionCall` parts in one response.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GeminiFunctionCallingMode {
    Auto,
    Any,
    None,
}

/// The `generationConfig` request parameter.
#[derive(Debug, Serialize, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    /// Caps *all* output tokens, thinking included — a thinking budget is
    /// spent out of this allowance, so it must be comfortably larger than the
    /// budget or the response is truncated with `finishReason: MAX_TOKENS`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<GeminiThinkingConfig>,
}

/// Extended-thinking configuration.
///
/// `includeThoughts` is what makes thought summaries appear in the stream;
/// without it the budget is still spent but nothing is streamed back, so the
/// two are always set together.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct GeminiThinkingConfig {
    /// Token budget for thinking. `0` disables thinking; `-1` asks the model to
    /// choose dynamically.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn gemini_request_serializes_to_native_wire_shape() {
        let request = GeminiRequest {
            contents: vec![
                GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiPart::Text {
                        text: "List /tmp".to_string(),
                    }],
                },
                GeminiContent {
                    role: GeminiRole::Model,
                    parts: vec![
                        GeminiPart::Thought {
                            text: "I should list the directory.".to_string(),
                            thought: true,
                            thought_signature: Some("sig-1".to_string()),
                        },
                        GeminiPart::FunctionCall {
                            function_call: GeminiFunctionCall {
                                name: "shell".to_string(),
                                args: json!({"command": ["ls", "/tmp"]}),
                            },
                        },
                    ],
                },
                GeminiContent {
                    role: GeminiRole::User,
                    parts: vec![GeminiPart::FunctionResponse {
                        function_response: GeminiFunctionResponse {
                            name: "shell".to_string(),
                            response: json!({"output": "a.txt"}),
                        },
                    }],
                },
            ],
            system_instruction: Some(GeminiSystemInstruction {
                parts: vec![GeminiPart::Text {
                    text: "You are Forall.".to_string(),
                }],
            }),
            tools: vec![GeminiTool {
                function_declarations: vec![GeminiFunctionDeclaration {
                    name: "shell".to_string(),
                    description: Some("Run a command".to_string()),
                    parameters: Some(json!({"type": "object"})),
                }],
            }],
            tool_config: Some(GeminiToolConfig {
                function_calling_config: GeminiFunctionCallingConfig {
                    mode: GeminiFunctionCallingMode::Auto,
                },
            }),
            generation_config: Some(GeminiGenerationConfig {
                max_output_tokens: Some(32_768),
                temperature: None,
                thinking_config: Some(GeminiThinkingConfig {
                    thinking_budget: Some(8_192),
                    include_thoughts: Some(true),
                }),
            }),
        };

        let expected = json!({
            "contents": [
                {
                    "role": "user",
                    "parts": [{"text": "List /tmp"}]
                },
                {
                    "role": "model",
                    "parts": [
                        {
                            "text": "I should list the directory.",
                            "thought": true,
                            "thoughtSignature": "sig-1"
                        },
                        {
                            "functionCall": {
                                "name": "shell",
                                "args": {"command": ["ls", "/tmp"]}
                            }
                        }
                    ]
                },
                {
                    "role": "user",
                    "parts": [
                        {
                            "functionResponse": {
                                "name": "shell",
                                "response": {"output": "a.txt"}
                            }
                        }
                    ]
                }
            ],
            "systemInstruction": {"parts": [{"text": "You are Forall."}]},
            "tools": [
                {
                    "functionDeclarations": [
                        {
                            "name": "shell",
                            "description": "Run a command",
                            "parameters": {"type": "object"}
                        }
                    ]
                }
            ],
            "toolConfig": {"functionCallingConfig": {"mode": "AUTO"}},
            "generationConfig": {
                "maxOutputTokens": 32768,
                "thinkingConfig": {"thinkingBudget": 8192, "includeThoughts": true}
            }
        });

        let actual = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(actual, expected);
    }

    #[test]
    fn empty_tools_and_optional_fields_are_omitted() {
        let request = GeminiRequest {
            contents: vec![GeminiContent {
                role: GeminiRole::User,
                parts: vec![GeminiPart::Text {
                    text: "hi".to_string(),
                }],
            }],
            system_instruction: None,
            tools: Vec::new(),
            tool_config: None,
            generation_config: None,
        };

        let actual = serde_json::to_value(&request).expect("serialize request");
        let expected = json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]}]
        });
        assert_eq!(actual, expected);
    }

    #[test]
    fn inline_data_part_serializes_camel_case_blob() {
        let part = GeminiPart::InlineData {
            inline_data: GeminiBlob {
                mime_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            },
        };

        assert_eq!(
            serde_json::to_value(&part).expect("serialize part"),
            json!({"inlineData": {"mimeType": "image/png", "data": "AAAA"}})
        );
    }

    #[test]
    fn thought_part_without_signature_omits_the_field() {
        let part = GeminiPart::Thought {
            text: "hmm".to_string(),
            thought: true,
            thought_signature: None,
        };

        assert_eq!(
            serde_json::to_value(&part).expect("serialize part"),
            json!({"text": "hmm", "thought": true})
        );
    }
}
