//! Internal backend contracts used by execution context implementations.

use crate::error::ToolError;
use async_trait::async_trait;

use super::types::{CapturePaneOptions, ExecOutput, SendKeysOptions, ShellWait, TmuxAttachInfo};

/// Internal backend trait used to decouple `ExecutionContext` from concrete
/// local/container/ssh implementations.
#[async_trait]
pub(super) trait ExecutionBackendOps: Send + Sync {
    /// Human-readable backend summary for status UI.
    fn summary(&self) -> String;
    /// Attach metadata when backend is tmux-backed.
    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo>;
    /// Startup pane reused from an existing managed session, if any.
    fn startup_existing_tmux_pane(&self) -> Option<String>;
    /// Whether capture-pane operations are supported.
    fn capture_pane_available(&self) -> bool;
    /// Capture tmux pane text according to options.
    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError>;
    /// Inject tmux keys/text according to options.
    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError>;
    /// Run a shell command with selected wait semantics.
    async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError>;
    /// Read file contents.
    async fn read_file(&self, path: &str) -> Result<String, ToolError>;
    /// Write file contents.
    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError>;
}

/// Shared contract for backends that can execute shell snippets.
#[async_trait]
pub(super) trait CommandBackend: Send + Sync {
    /// Execute one shell command with optional stdin and wait behavior.
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError>;
}
