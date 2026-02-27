//! File read/write tools.
//!
//! - `read_file`: reads a file's contents (truncated if large).
//! - `write_file`: writes content to a file, creating it if needed.

use async_trait::async_trait;
use serde::Deserialize;

use super::execution::ExecutionContext;
use super::Tool;
use crate::error::ToolError;
use crate::textutil::truncate_with_suffix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters to return when reading a file.
const MAX_READ_LEN: usize = 8000;

// ---------------------------------------------------------------------------
// ReadFile
// ---------------------------------------------------------------------------

/// Tool that reads the contents of a file.
pub struct ReadFileTool {
    /// Where file reads are executed (local/container/ssh).
    pub execution: ExecutionContext,
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Read the contents of a file at the given path.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to read"
                        }
                    },
                    "required": ["path"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: ReadArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        let content = self.execution.read_file(&args.path).await?;

        if content.len() > MAX_READ_LEN {
            Ok(truncate_with_suffix_by_bytes(
                &content,
                MAX_READ_LEN,
                "...[truncated]",
            ))
        } else {
            Ok(content)
        }
    }
}

// ---------------------------------------------------------------------------
// WriteFile
// ---------------------------------------------------------------------------

/// Tool that writes content to a file.
pub struct WriteFileTool {
    /// Where file writes are executed (local/container/ssh).
    pub execution: ExecutionContext,
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Write content to a file at the given path. Creates the file if it doesn't exist, overwrites if it does.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path to the file to write"
                        },
                        "content": {
                            "type": "string",
                            "description": "Content to write to the file"
                        }
                    },
                    "required": ["path", "content"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: WriteArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

        self.execution.write_file(&args.path, &args.content).await?;

        Ok(format!(
            "Wrote {} bytes to {}",
            args.content.len(),
            args.path
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::TestTempDir;

    #[test]
    fn read_tool_name() {
        assert_eq!(
            ReadFileTool {
                execution: ExecutionContext::local()
            }
            .name(),
            "read_file"
        );
    }

    #[test]
    fn write_tool_name() {
        assert_eq!(
            WriteFileTool {
                execution: ExecutionContext::local()
            }
            .name(),
            "write_file"
        );
    }

    #[tokio::test]
    async fn read_invalid_json_returns_error() {
        let err = ReadFileTool {
            execution: ExecutionContext::local(),
        }
        .execute("not json")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid arguments"));
    }

    #[tokio::test]
    async fn read_nonexistent_file_returns_error() {
        let err = ReadFileTool {
            execution: ExecutionContext::local(),
        }
        .execute(r#"{"path": "/tmp/agent_no_such_file_xyz.txt"}"#)
        .await
        .unwrap_err();
        assert!(err.to_string().contains("execution failed"));
    }

    #[tokio::test]
    async fn read_file_returns_contents() {
        let fixture = TestTempDir::new("read-file");
        let path = fixture.path().join("file.txt");
        tokio::fs::write(&path, "file content").await.unwrap();
        let args = format!(r#"{{"path": "{}"}}"#, path.display());
        let result = ReadFileTool {
            execution: ExecutionContext::local(),
        }
        .execute(&args)
        .await
        .unwrap();
        assert_eq!(result, "file content");
    }

    #[tokio::test]
    async fn read_file_truncates_large_content() {
        let fixture = TestTempDir::new("read-file-large");
        let path = fixture.path().join("large.txt");
        let big = "x".repeat(MAX_READ_LEN + 100);
        tokio::fs::write(&path, &big).await.unwrap();
        let args = format!(r#"{{"path": "{}"}}"#, path.display());
        let result = ReadFileTool {
            execution: ExecutionContext::local(),
        }
        .execute(&args)
        .await
        .unwrap();
        assert!(result.ends_with("...[truncated]"), "got: {result}");
    }

    #[tokio::test]
    async fn read_file_truncation_is_utf8_safe() {
        let fixture = TestTempDir::new("read-file-utf8");
        let path = fixture.path().join("utf8.txt");
        let big = "🙂".repeat(MAX_READ_LEN + 10);
        tokio::fs::write(&path, &big).await.unwrap();
        let args = format!(r#"{{"path": "{}"}}"#, path.display());
        let result = ReadFileTool {
            execution: ExecutionContext::local(),
        }
        .execute(&args)
        .await
        .unwrap();
        assert!(result.ends_with("...[truncated]"), "got: {result}");
    }

    #[tokio::test]
    async fn write_invalid_json_returns_error() {
        let err = WriteFileTool {
            execution: ExecutionContext::local(),
        }
        .execute("not json")
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid arguments"));
    }

    #[tokio::test]
    async fn write_file_creates_and_reports_bytes() {
        let fixture = TestTempDir::new("write-file");
        let path = fixture.path().join("written.txt");
        let content = "hello write";
        let args = format!(
            r#"{{"path": "{}", "content": "{content}"}}"#,
            path.display()
        );
        let result = WriteFileTool {
            execution: ExecutionContext::local(),
        }
        .execute(&args)
        .await
        .unwrap();
        assert!(result.contains(&content.len().to_string()), "got: {result}");
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(written, content);
    }
}
