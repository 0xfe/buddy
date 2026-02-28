//! Pluggable tool system.
//!
//! Tools are async trait objects that the model can invoke during the agentic
//! loop. Each tool provides its own OpenAI function definition and an async
//! execute method.

pub mod capture_pane;
pub mod execution;
pub mod fetch;
pub mod files;
pub mod result_envelope;
pub mod search;
pub mod send_keys;
pub mod shell;
pub mod time;

use crate::error::ToolError;
use crate::types::ToolDefinition;
use async_trait::async_trait;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// A tool that can be invoked by the AI model.
///
/// Implement this trait to add custom tools. Register instances with
/// [`ToolRegistry`] before creating the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name matching what the model will call.
    fn name(&self) -> &'static str;

    /// OpenAI-format tool definition for inclusion in API requests.
    fn definition(&self) -> ToolDefinition;

    /// Execute the tool with the given JSON arguments string.
    /// Returns a text result to send back to the model.
    async fn execute(&self, arguments: &str, context: &ToolContext) -> Result<String, ToolError>;
}

/// Incremental tool output emitted while a tool is running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStreamEvent {
    Started { detail: String },
    StdoutChunk { chunk: String },
    StderrChunk { chunk: String },
    Info { message: String },
    Completed { detail: String },
}

/// Runtime context passed to tools.
///
/// This keeps the tool trait simple while giving tools an optional streaming
/// side channel for incremental output.
#[derive(Clone, Default)]
pub struct ToolContext {
    stream_tx: Option<mpsc::UnboundedSender<ToolStreamEvent>>,
}

impl ToolContext {
    /// Build a no-op context (no streaming sink attached).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Build a context that forwards streaming events to a receiver.
    pub fn with_stream(stream_tx: mpsc::UnboundedSender<ToolStreamEvent>) -> Self {
        Self {
            stream_tx: Some(stream_tx),
        }
    }

    /// True when the caller attached a streaming event sink.
    ///
    /// Runtime-driven interactive mode uses this to render tool progress via
    /// centralized task status UI instead of per-tool spinner threads.
    pub fn has_stream(&self) -> bool {
        self.stream_tx.is_some()
    }

    /// Emit one stream event if a sink is attached.
    pub fn emit(&self, event: ToolStreamEvent) {
        if let Some(tx) = &self.stream_tx {
            let _ = tx.send(event);
        }
    }
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

/// Registry of available tools.
///
/// The agent sends all registered tool definitions to the API, and dispatches
/// tool calls through this registry.
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
    }

    /// Get tool definitions for the API request.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    /// Find a tool by name and execute it.
    pub async fn execute(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.execute_with_context(name, arguments, &ToolContext::empty())
            .await
    }

    /// Find a tool by name and execute it with streaming context.
    pub async fn execute_with_context(
        &self,
        name: &str,
        arguments: &str,
        context: &ToolContext,
    ) -> Result<String, ToolError> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.name() == name)
            .ok_or_else(|| ToolError::ExecutionFailed(format!("unknown tool: {name}")))?;
        tool.execute(arguments, context).await
    }

    /// True if no tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// True when a tool with the given name is registered.
    pub fn has_tool(&self, name: &str) -> bool {
        self.tools.iter().any(|tool| tool.name() == name)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::FunctionDefinition;
    use async_trait::async_trait;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo"
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                tool_type: "function".into(),
                function: FunctionDefinition {
                    name: "echo".into(),
                    description: "echoes arguments back".into(),
                    parameters: serde_json::json!({}),
                },
            }
        }
        async fn execute(
            &self,
            arguments: &str,
            _context: &ToolContext,
        ) -> Result<String, ToolError> {
            Ok(arguments.to_string())
        }
    }

    #[test]
    fn new_registry_is_empty() {
        assert!(ToolRegistry::new().is_empty());
    }

    #[test]
    fn default_registry_is_empty() {
        assert!(ToolRegistry::default().is_empty());
    }

    #[test]
    fn register_makes_nonempty() {
        let mut r = ToolRegistry::new();
        r.register(EchoTool);
        assert!(!r.is_empty());
    }

    #[test]
    fn definitions_returns_registered_tools() {
        let mut r = ToolRegistry::new();
        r.register(EchoTool);
        let defs = r.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "echo");
    }

    #[tokio::test]
    async fn execute_known_tool_returns_output() {
        let mut r = ToolRegistry::new();
        r.register(EchoTool);
        let out = r.execute("echo", r#"{"x":1}"#).await.unwrap();
        assert_eq!(out, r#"{"x":1}"#);
    }

    #[tokio::test]
    async fn execute_unknown_tool_returns_error() {
        let r = ToolRegistry::new();
        let err = r.execute("nonexistent", "{}").await.unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }
}
