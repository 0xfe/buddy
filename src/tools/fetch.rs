//! URL fetch tool.
//!
//! Performs an async HTTP GET and returns the response body as text.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::Tool;
use crate::error::ToolError;
use crate::textutil::truncate_with_suffix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters of response body to return.
const MAX_BODY_LEN: usize = 8000;
const DEFAULT_FETCH_TIMEOUT_SECS: u64 = 20;

/// Tool that fetches a URL and returns its text content.
pub struct FetchTool {
    http: reqwest::Client,
}

impl FetchTool {
    pub fn new(timeout: Duration) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { http }
    }
}

impl Default for FetchTool {
    fn default() -> Self {
        Self::new(Duration::from_secs(DEFAULT_FETCH_TIMEOUT_SECS))
    }
}

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

        let body = self
            .http
            .get(&args.url)
            .send()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
            .text()
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(truncate_body(&body))
    }
}

fn truncate_body(body: &str) -> String {
    truncate_with_suffix_by_bytes(body, MAX_BODY_LEN, "...[truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn truncate_body_keeps_short_text() {
        assert_eq!(truncate_body("hello"), "hello");
    }

    #[test]
    fn truncate_body_is_utf8_safe() {
        let body = "🙂".repeat(MAX_BODY_LEN + 5);
        let out = truncate_body(&body);
        assert!(out.ends_with("...[truncated]"), "got: {out}");
    }

    #[tokio::test]
    async fn fetch_tool_respects_configured_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept one connection and intentionally never send an HTTP response.
        let _accept = tokio::spawn(async move {
            let _ = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let tool = FetchTool::new(Duration::from_millis(50));
        let args = format!(r#"{{"url":"http://{addr}/hang"}}"#);
        let err = tool.execute(&args).await.expect_err("timeout expected");
        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("timed out") || msg.contains("timeout"),
            "unexpected error: {msg}"
        );
    }
}
