//! Internal backend contracts used by execution context implementations.

use crate::error::ToolError;
use async_trait::async_trait;

use super::types::{CapturePaneOptions, ExecOutput, SendKeysOptions, ShellWait, TmuxAttachInfo};

/// Internal backend trait used to decouple `ExecutionContext` from concrete
/// local/container/ssh implementations.
#[async_trait]
pub(super) trait ExecutionBackendOps: Send + Sync {
    fn summary(&self) -> String;
    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo>;
    fn startup_existing_tmux_pane(&self) -> Option<String>;
    fn capture_pane_available(&self) -> bool;
    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError>;
    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError>;
    async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError>;
    async fn read_file(&self, path: &str) -> Result<String, ToolError>;
    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError>;
}

/// Shared contract for backends that can execute shell snippets.
#[async_trait]
pub(super) trait CommandBackend: Send + Sync {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError>;
}
