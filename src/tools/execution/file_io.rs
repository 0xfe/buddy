//! File read/write helpers routed through command-capable backends.

use crate::error::ToolError;

use super::contracts::CommandBackend;
use super::process::{ensure_success, shell_quote};
use super::types::ShellWait;

/// Read a file via backend shell command execution.
pub(super) async fn read_file_via_command_backend(
    backend: &(impl CommandBackend + ?Sized),
    path: &str,
) -> Result<String, ToolError> {
    let script = format!("cat -- {}", shell_quote(path));
    let output = backend.run_command(&script, None, ShellWait::Wait).await?;
    ensure_success(output, path.to_string()).map(|out| out.stdout)
}

/// Write a file via backend shell command execution.
pub(super) async fn write_file_via_command_backend(
    backend: &(impl CommandBackend + ?Sized),
    path: &str,
    content: &str,
) -> Result<(), ToolError> {
    let script = format!("cat > {}", shell_quote(path));
    let output = backend
        .run_command(&script, Some(content.as_bytes()), ShellWait::Wait)
        .await?;
    ensure_success(output, path.to_string()).map(|_| ())
}
