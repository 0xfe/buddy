//! Local backend implementations and tmux-environment helpers.

use crate::error::ToolError;
use async_trait::async_trait;

use crate::tmux::capture::run_local_capture_pane;
use crate::tmux::pane::ensure_local_tmux_pane;
use crate::tmux::prompt::ensure_local_tmux_prompt_setup;
use crate::tmux::run::run_local_tmux_process;
use crate::tmux::send_keys::{send_local_tmux_keys, send_local_tmux_line};
use crate::tools::execution::contracts::{CommandBackend, ExecutionBackendOps};
use crate::tools::execution::file_io::{
    read_file_via_command_backend, write_file_via_command_backend,
};
use crate::tools::execution::process::{run_sh_process, run_with_wait, shell_quote};
use crate::tools::execution::types::{
    CapturePaneOptions, ExecOutput, LocalBackend, LocalTmuxContext, SendKeysOptions, ShellWait,
    TmuxAttachInfo, TmuxAttachTarget, LEGACY_TMUX_WINDOW_NAME, TMUX_PANE_TITLE, TMUX_WINDOW_NAME,
};

impl LocalTmuxContext {
    pub(in crate::tools::execution) async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        let pane_id = self.ensure_prompt_ready().await?;
        run_local_tmux_process(&pane_id, command, stdin, wait).await
    }

    pub(in crate::tools::execution) async fn ensure_prompt_ready(
        &self,
    ) -> Result<String, ToolError> {
        let configured_pane = self.configured_tmux_pane.lock().await.clone();
        if let Some(pane_id) = configured_pane {
            if local_tmux_pane_exists(&pane_id).await? {
                return Ok(pane_id);
            }
        }

        let ensured = ensure_local_tmux_pane(&self.tmux_session).await?;
        let mut configured = self.configured_tmux_pane.lock().await;
        if ensured.created {
            ensure_local_tmux_prompt_setup(&ensured.pane_id).await?;
        }
        if configured.as_deref() != Some(ensured.pane_id.as_str()) {
            *configured = Some(ensured.pane_id.clone());
        }
        Ok(ensured.pane_id)
    }
}

async fn local_tmux_pane_exists(pane_id: &str) -> Result<bool, ToolError> {
    let pane_q = shell_quote(pane_id);
    let probe =
        format!("tmux list-panes -a -F '#{{pane_id}}' | grep -Fx -- {pane_q} >/dev/null 2>&1");
    let output = run_sh_process("sh", &probe, None).await?;
    Ok(output.exit_code == 0)
}

#[async_trait]
impl CommandBackend for LocalTmuxContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        LocalTmuxContext::run_command(self, command, stdin, wait).await
    }
}

#[async_trait]
impl ExecutionBackendOps for LocalBackend {
    fn summary(&self) -> String {
        "local".to_string()
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        None
    }

    fn startup_existing_tmux_pane(&self) -> Option<String> {
        None
    }

    fn capture_pane_available(&self) -> bool {
        local_tmux_pane_target().is_some()
    }

    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        let pane_target = options
            .target
            .as_deref()
            .map(str::to_string)
            .or_else(local_tmux_pane_target)
            .ok_or_else(|| {
                ToolError::ExecutionFailed("capture-pane requires an active tmux session".into())
            })?;
        run_local_capture_pane(&pane_target, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
        let pane_target = options
            .target
            .as_deref()
            .map(str::to_string)
            .or_else(local_tmux_pane_target)
            .ok_or_else(|| {
                ToolError::ExecutionFailed("send-keys requires an active tmux session".into())
            })?;
        send_local_tmux_keys(&pane_target, &options).await?;
        Ok(format!("sent keys to tmux pane {pane_target}"))
    }

    async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        if matches!(wait, ShellWait::NoWait) {
            let pane_id = local_tmux_pane_target().ok_or_else(|| {
                ToolError::ExecutionFailed(
                    "run_shell wait=false requires an active tmux session".into(),
                )
            })?;
            send_local_tmux_line(&pane_id, command).await?;
            return Ok(ExecOutput {
                exit_code: 0,
                stdout: format!(
                    "command dispatched to tmux pane {pane_id}; still running in background. Use capture-pane (optionally with delay) to poll output."
                ),
                stderr: String::new(),
            });
        }
        run_with_wait(
            run_sh_process("sh", command, None),
            wait,
            "timed out waiting for local command completion",
        )
        .await
    }

    async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("{path}: {e}")))
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("{path}: {e}")))
    }
}

#[async_trait]
impl ExecutionBackendOps for LocalTmuxContext {
    fn summary(&self) -> String {
        format!("local (tmux:{})", self.tmux_session)
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        Some(TmuxAttachInfo {
            session: self.tmux_session.clone(),
            window: TMUX_WINDOW_NAME,
            target: TmuxAttachTarget::Local,
        })
    }

    fn startup_existing_tmux_pane(&self) -> Option<String> {
        self.startup_existing_tmux_pane.clone()
    }

    fn capture_pane_available(&self) -> bool {
        true
    }

    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready().await?
        };
        run_local_capture_pane(&pane_target, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready().await?
        };
        send_local_tmux_keys(&pane_target, &options).await?;
        Ok(format!("sent keys to tmux pane {pane_target}"))
    }

    async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        self.run_command(command, None, wait).await
    }

    async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        read_file_via_command_backend(self, path).await
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        write_file_via_command_backend(self, path, content).await
    }
}

pub(in crate::tools::execution) async fn ensure_not_in_managed_local_tmux_pane(
) -> Result<(), ToolError> {
    let Some(current_pane) = local_tmux_pane_target() else {
        return Ok(());
    };
    let pane_q = shell_quote(&current_pane);
    let inspect =
        format!("tmux display-message -p -t {pane_q} '#{{pane_title}}\n#{{window_name}}'");
    let output = run_sh_process("sh", &inspect, None).await?;
    let output = crate::tools::execution::process::ensure_success(
        output,
        "failed to inspect current tmux pane".into(),
    )?;
    let mut lines = output.stdout.lines();
    let pane_title = lines.next().unwrap_or_default().trim();
    let window_name = lines.next().unwrap_or_default().trim();
    if pane_title == TMUX_PANE_TITLE
        || (pane_title.is_empty() && window_name == LEGACY_TMUX_WINDOW_NAME)
    {
        return Err(ToolError::ExecutionFailed(
            "buddy should be run from a different terminal when --tmux is enabled (current pane is shared)".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::tools::execution) fn is_managed_tmux_window_name(window_name: &str) -> bool {
    let normalized = window_name.trim();
    normalized == TMUX_WINDOW_NAME || normalized == LEGACY_TMUX_WINDOW_NAME
}

pub(in crate::tools::execution) fn local_tmux_pane_target() -> Option<String> {
    if !local_tmux_allowed() {
        return None;
    }

    let pane = std::env::var("TMUX_PANE").ok()?;
    if pane.trim().is_empty() {
        None
    } else {
        Some(pane)
    }
}

pub(in crate::tools::execution) fn local_tmux_allowed() -> bool {
    #[cfg(test)]
    {
        std::env::var("BUDDY_TEST_USE_REAL_TMUX")
            .or_else(|_| std::env::var("AGENT_TEST_USE_REAL_TMUX"))
            .ok()
            .is_some_and(|v| v.trim() == "1")
    }

    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_tmux_window_name_detection_accepts_new_and_legacy_names() {
        assert!(is_managed_tmux_window_name("shared"));
        assert!(is_managed_tmux_window_name(" shared "));
        assert!(is_managed_tmux_window_name("buddy-shared"));
        assert!(is_managed_tmux_window_name(" buddy-shared "));
        assert!(!is_managed_tmux_window_name("dev-shell"));
    }

    #[test]
    fn local_tmux_is_disabled_by_default_in_unit_tests() {
        assert!(!local_tmux_allowed());
        assert!(local_tmux_pane_target().is_none());
    }
}
