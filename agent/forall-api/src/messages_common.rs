//! Request-side types for the Anthropic Messages API.
//!
//! These structs serialize directly into the `POST /v1/messages` JSON body —
//! like [`crate::common::ResponsesApiRequest`], the `Serialize` impls *are*
//! the wire format, with no intermediate DTO.

use serde::Serialize;
use serde_json::Value;

/// JSON body for a streaming `POST /v1/messages` request.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MessagesApiRequest {
    pub model: String,
    /// System prompt, expressed as typed blocks so cache breakpoints can be
    /// attached to the stable prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<MessagesSystemBlock>>,
    pub messages: Vec<MessagesMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<MessagesToolSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<MessagesToolChoice>,
    /// Required by the Messages API; the response is truncated with
    /// `stop_reason: "max_tokens"` when reached.
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MessagesMetadata>,
}

/// One block of the `system` array: `{"type": "text", ...}`.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagesSystemBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

/// A single conversation turn in the `messages` array.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MessagesMessage {
    pub role: MessagesRole,
    pub content: Vec<MessagesContentBlock>,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessagesRole {
    User,
    Assistant,
}

/// Content blocks accepted (and emitted) by the Messages API.
///
/// `thinking`/`redacted_thinking` blocks must round-trip byte-exact on replay:
/// Anthropic rejects requests whose `signature` does not match the `thinking`
/// text, so the request builder reconstructs them from the envelope stored in
/// `ResponseItem::Reasoning::encrypted_content`.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagesContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Image {
        source: MessagesImageSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        /// Tool input as a JSON *object* (unlike the Responses API, which uses
        /// a JSON-encoded string).
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolResult {
        tool_use_id: String,
        content: Vec<MessagesToolResultContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

/// Content nested inside a `tool_result` block.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagesToolResultContent {
    Text { text: String },
    Image { source: MessagesImageSource },
}

/// Image payload for `image` blocks.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagesImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

/// Tool definition: `{name, description, input_schema}`.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MessagesToolSpec {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// The `tool_choice` request parameter.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MessagesToolChoice {
    Auto {
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    Any {
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    Tool {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        disable_parallel_tool_use: Option<bool>,
    },
    None,
}

/// Extended-thinking configuration: `{"type": "enabled", "budget_tokens": N}`.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Enabled { budget_tokens: u32 },
}

/// Explicit prompt-cache breakpoint: `{"type": "ephemeral"}`.
///
/// Unlike the Responses API's opaque `prompt_cache_key`, Anthropic caching is
/// positional — at most 4 blocks may carry a breakpoint, and only content
/// before a breakpoint is cached.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheControl {
    Ephemeral,
}

/// Request metadata (`metadata.user_id`).
#[derive(Debug, Serialize, Clone, PartialEq, Eq, Default)]
pub struct MessagesMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn messages_api_request_serializes_to_anthropic_wire_shape() {
        let request = MessagesApiRequest {
            model: "claude-sonnet-4-5".to_string(),
            system: Some(vec![MessagesSystemBlock::Text {
                text: "You are Forall.".to_string(),
                cache_control: Some(CacheControl::Ephemeral),
            }]),
            messages: vec![
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![MessagesContentBlock::Text {
                        text: "List /tmp".to_string(),
                        cache_control: None,
                    }],
                },
                MessagesMessage {
                    role: MessagesRole::Assistant,
                    content: vec![
                        MessagesContentBlock::Thinking {
                            thinking: "I should list the directory.".to_string(),
                            signature: "sig-1".to_string(),
                        },
                        MessagesContentBlock::ToolUse {
                            id: "toolu_1".to_string(),
                            name: "shell".to_string(),
                            input: json!({"command": ["ls", "/tmp"]}),
                            cache_control: None,
                        },
                    ],
                },
                MessagesMessage {
                    role: MessagesRole::User,
                    content: vec![MessagesContentBlock::ToolResult {
                        tool_use_id: "toolu_1".to_string(),
                        content: vec![MessagesToolResultContent::Text {
                            text: "a.txt".to_string(),
                        }],
                        is_error: None,
                        cache_control: None,
                    }],
                },
            ],
            tools: vec![MessagesToolSpec {
                name: "shell".to_string(),
                description: Some("Run a command".to_string()),
                input_schema: json!({"type": "object"}),
                cache_control: Some(CacheControl::Ephemeral),
            }],
            tool_choice: Some(MessagesToolChoice::Auto {
                disable_parallel_tool_use: Some(true),
            }),
            max_tokens: 8192,
            thinking: Some(ThinkingConfig::Enabled {
                budget_tokens: 4096,
            }),
            stream: true,
            metadata: Some(MessagesMetadata {
                user_id: Some("thread-1".to_string()),
            }),
        };

        let expected = json!({
            "model": "claude-sonnet-4-5",
            "system": [
                {"type": "text", "text": "You are Forall.", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [{"type": "text", "text": "List /tmp"}]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "I should list the directory.",
                            "signature": "sig-1"
                        },
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "shell",
                            "input": {"command": ["ls", "/tmp"]}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "tool_result",
                            "tool_use_id": "toolu_1",
                            "content": [{"type": "text", "text": "a.txt"}]
                        }
                    ]
                }
            ],
            "tools": [
                {
                    "name": "shell",
                    "description": "Run a command",
                    "input_schema": {"type": "object"},
                    "cache_control": {"type": "ephemeral"}
                }
            ],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
            "max_tokens": 8192,
            "thinking": {"type": "enabled", "budget_tokens": 4096},
            "stream": true,
            "metadata": {"user_id": "thread-1"}
        });

        let actual = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(actual, expected);
    }

    #[test]
    fn empty_tools_and_optional_fields_are_omitted() {
        let request = MessagesApiRequest {
            model: "claude-haiku-4-5".to_string(),
            system: None,
            messages: vec![MessagesMessage {
                role: MessagesRole::User,
                content: vec![MessagesContentBlock::Text {
                    text: "hi".to_string(),
                    cache_control: None,
                }],
            }],
            tools: vec![],
            tool_choice: None,
            max_tokens: 1024,
            thinking: None,
            stream: true,
            metadata: None,
        };

        let actual = serde_json::to_value(&request).expect("serialize request");
        let expected = json!({
            "model": "claude-haiku-4-5",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "hi"}]}
            ],
            "max_tokens": 1024,
            "stream": true
        });
        assert_eq!(actual, expected);
    }
}
