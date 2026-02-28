//! Data model for the OpenAI Chat Completions API.
//!
//! These types are designed to serialize/deserialize directly to/from the
//! JSON payloads expected by any OpenAI-compatible endpoint.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Message roles
// ---------------------------------------------------------------------------

/// Conversation participant role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System instruction message.
    System,
    /// End-user message.
    User,
    /// Assistant/model message.
    Assistant,
    /// Tool execution result message.
    Tool,
}

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

/// A single message in the conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Author role for this conversation turn.
    pub role: Role,

    /// Text content. Null when the assistant message is purely tool calls.
    pub content: Option<String>,

    /// Tool calls requested by the assistant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,

    /// When role == Tool, the id of the tool_call this result corresponds to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    /// Optional name (used in some APIs for tool responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Provider-specific message fields that should be preserved verbatim.
    ///
    /// This keeps compatibility with OpenAI-compatible APIs that attach extra
    /// data (for example reasoning metadata) and expect it back on follow-up
    /// tool-call turns.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty", flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl Message {
    /// Create a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }
    }

    /// Create a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }
    }

    /// Create a tool result message, sent back after executing a tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            name: None,
            extra: BTreeMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool calls (in assistant responses)
// ---------------------------------------------------------------------------

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique id used to correlate tool call and tool result.
    pub id: String,
    /// Tool call type; currently expected to be `"function"`.
    #[serde(rename = "type")]
    pub call_type: String, // "function"
    /// Function metadata and arguments for this tool invocation.
    pub function: FunctionCall,
}

/// The function name and JSON-encoded arguments within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function/tool name to execute.
    pub name: String,
    /// JSON-encoded string of the arguments object.
    pub arguments: String,
}

// ---------------------------------------------------------------------------
// Tool definitions (sent in requests)
// ---------------------------------------------------------------------------

/// Tool definition included in the API request so the model knows what's available.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Tool definition type; currently expected to be `"function"`.
    #[serde(rename = "type")]
    pub tool_type: String, // "function"
    /// Function schema published to the model.
    pub function: FunctionDefinition,
}

/// The schema of a callable function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    /// Exposed function/tool name.
    pub name: String,
    /// Natural-language description of tool behavior.
    pub description: String,
    /// JSON Schema object describing the parameters.
    pub parameters: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Chat completion request / response
// ---------------------------------------------------------------------------

/// Request body for POST /chat/completions.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    /// Model identifier used for request routing.
    pub model: String,
    /// Conversation history sent to the model.
    pub messages: Vec<Message>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
}

/// Response body from POST /chat/completions.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    /// Provider response id.
    pub id: String,
    /// Ranked response choices.
    pub choices: Vec<Choice>,
    /// Optional token usage metadata.
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// A single choice in the API response.
#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    /// Choice index in the provider response.
    pub index: u32,
    /// Assistant message payload for this choice.
    pub message: Message,
    /// Provider stop reason (`stop`, `tool_calls`, etc.).
    pub finish_reason: Option<String>,
}

/// Token usage reported by the API.
#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    /// Input tokens consumed by the request.
    pub prompt_tokens: u64,
    /// Output tokens generated by the model.
    pub completion_tokens: u64,
    /// Total tokens (`prompt + completion`).
    pub total_tokens: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies optional fields are omitted when absent during request serialization.
    #[test]
    fn serialize_chat_request() {
        let req = ChatRequest {
            model: "gpt-4o".into(),
            messages: vec![Message::system("You are helpful."), Message::user("Hi")],
            tools: None,
            temperature: Some(0.7),
            top_p: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "gpt-4o");
        assert_eq!(json["messages"].as_array().unwrap().len(), 2);
        assert_eq!(json["temperature"], 0.7);
        // top_p should be omitted
        assert!(json.get("top_p").is_none());
        // tools should be omitted
        assert!(json.get("tools").is_none());
    }

    // Verifies standard assistant text responses deserialize correctly.
    #[test]
    fn deserialize_chat_response() {
        let json = r#"{
            "id": "chatcmpl-123",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "chatcmpl-123");
        assert_eq!(resp.choices.len(), 1);
        assert_eq!(resp.choices[0].message.content.as_deref(), Some("Hello!"));
        assert_eq!(resp.usage.as_ref().unwrap().total_tokens, 15);
    }

    // Verifies assistant tool-call responses deserialize with null content.
    #[test]
    fn deserialize_tool_call_response() {
        let json = r#"{
            "id": "chatcmpl-456",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "run_shell",
                            "arguments": "{\"command\":\"ls\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let msg = &resp.choices[0].message;
        assert!(msg.content.is_none());
        let tc = msg.tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].function.name, "run_shell");
    }

    #[test]
    fn preserves_provider_specific_message_fields() {
        let json = r#"{
            "id": "chatcmpl-789",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "reasoning_content": "thinking trace",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "run_shell",
                            "arguments": "{\"command\":\"ls\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;

        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let req = ChatRequest {
            model: "kimi-k2".into(),
            messages: vec![resp.choices[0].message.clone()],
            tools: None,
            temperature: None,
            top_p: None,
        };
        let out = serde_json::to_value(req).unwrap();

        assert!(out["messages"][0]["content"].is_null());
        assert_eq!(out["messages"][0]["reasoning_content"], "thinking trace");
    }

    #[test]
    fn message_constructors() {
        let sys = Message::system("hello");
        assert_eq!(sys.role, Role::System);
        assert_eq!(sys.content.as_deref(), Some("hello"));

        let usr = Message::user("world");
        assert_eq!(usr.role, Role::User);

        let tool = Message::tool_result("call_1", "result data");
        assert_eq!(tool.role, Role::Tool);
        assert_eq!(tool.tool_call_id.as_deref(), Some("call_1"));
        assert!(tool.extra.is_empty());
    }
}
