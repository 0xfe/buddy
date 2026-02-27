//! File read/write tools.
//!
//! - `read_file`: reads a file's contents (truncated if large).
//! - `write_file`: writes content to a file, creating it if needed.

use async_trait::async_trait;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

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
    /// Optional root path allowlist for writes.
    pub allowed_paths: Vec<String>,
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
        validate_write_path_policy(&args.path, &self.allowed_paths)?;

        self.execution.write_file(&args.path, &args.content).await?;

        Ok(format!(
            "Wrote {} bytes to {}",
            args.content.len(),
            args.path
        ))
    }
}

fn validate_write_path_policy(path: &str, allowed_paths: &[String]) -> Result<(), ToolError> {
    let target = normalize_target_path(path)?;
    let allowed = normalize_allowed_paths(allowed_paths);
    let explicitly_allowed = allowed.iter().any(|root| target.starts_with(root));

    if !allowed.is_empty() && !explicitly_allowed {
        return Err(ToolError::ExecutionFailed(format!(
            "write_file blocked: path `{}` is outside tools.files_allowed_paths",
            target.display()
        )));
    }

    let sensitive = sensitive_roots();
    let blocked_sensitive = sensitive.iter().any(|root| target.starts_with(root));
    if blocked_sensitive && !explicitly_allowed {
        return Err(ToolError::ExecutionFailed(format!(
            "write_file blocked: path `{}` is under a sensitive system directory",
            target.display()
        )));
    }
    Ok(())
}

fn normalize_target_path(raw: &str) -> Result<PathBuf, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidArguments(
            "path must be a non-empty string".to_string(),
        ));
    }
    let path = PathBuf::from(trimmed);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| ToolError::ExecutionFailed(format!("failed to resolve current dir: {e}")))?
            .join(path)
    };
    Ok(normalize_lexical(&absolute))
}

fn normalize_allowed_paths(paths: &[String]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|raw| normalize_target_path(raw).ok())
        .collect()
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir => out.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
        }
    }
    out
}

fn sensitive_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        normalize_lexical(Path::new("/etc")),
        normalize_lexical(Path::new("/bin")),
        normalize_lexical(Path::new("/sbin")),
        normalize_lexical(Path::new("/usr")),
        normalize_lexical(Path::new("/boot")),
        normalize_lexical(Path::new("/dev")),
        normalize_lexical(Path::new("/proc")),
        normalize_lexical(Path::new("/sys")),
        normalize_lexical(Path::new("/System")),
        normalize_lexical(Path::new("/Library")),
        normalize_lexical(Path::new("/private/etc")),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(normalize_lexical(&home.join(".ssh")));
        roots.push(normalize_lexical(&home.join(".gnupg")));
        roots.push(normalize_lexical(&home.join(".aws")));
    }
    roots
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
                execution: ExecutionContext::local(),
                allowed_paths: Vec::new(),
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
            allowed_paths: Vec::new(),
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
            allowed_paths: vec![fixture.path().display().to_string()],
        }
        .execute(&args)
        .await
        .unwrap();
        assert!(result.contains(&content.len().to_string()), "got: {result}");
        let written = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(written, content);
    }

    #[test]
    fn write_policy_blocks_sensitive_path_by_default() {
        let err = validate_write_path_policy("/etc/passwd", &[]).expect_err("should be blocked");
        assert!(
            err.to_string().contains("sensitive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn write_policy_allows_sensitive_path_when_explicitly_allowlisted() {
        assert!(validate_write_path_policy("/etc/buddy-test.conf", &["/etc".to_string()]).is_ok());
    }

    #[test]
    fn write_policy_blocks_paths_outside_allowlist() {
        let fixture = TestTempDir::new("write-policy");
        let allowed = fixture.path().join("allowed");
        let denied = fixture.path().join("denied").join("x.txt");
        let err = validate_write_path_policy(
            denied.to_string_lossy().as_ref(),
            &[allowed.to_string_lossy().to_string()],
        )
        .expect_err("outside allowlist should be blocked");
        assert!(
            err.to_string().contains("files_allowed_paths"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn write_policy_allows_paths_inside_allowlist() {
        let fixture = TestTempDir::new("write-policy-ok");
        let allowed = fixture.path().join("allowed");
        let target = allowed.join("subdir").join("x.txt");
        assert!(validate_write_path_policy(
            target.to_string_lossy().as_ref(),
            &[allowed.to_string_lossy().to_string()]
        )
        .is_ok());
    }
}
