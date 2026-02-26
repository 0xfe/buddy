//! URL fetch tool.
//!
//! Performs an async HTTP GET and returns the response body as text.

use async_trait::async_trait;
use serde::Deserialize;

use super::Tool;
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters of response body to return.
const MAX_BODY_LEN: usize = 8000;

/// Tool that fetches a URL and returns its text content.
pub struct FetchTool;

#[derive(Deserialize)]
struct Args {
    url: String,
}

#[async_trait]
impl Tool for FetchTool {
    fn name(&self) -> &'static str {
        "fetch_url"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Fetch the contents of a URL and return the response body as text."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "The URL to fetch"
                        }
                    },
                    "required": ["url"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let body = reqwest::get(&args.url)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if body.len() > MAX_BODY_LEN {
            Ok(format!("{}...[truncated]", &body[..MAX_BODY_LEN]))
        } else {
            Ok(body)
        }
    }
}
