//! Container backend implementations (raw and tmux-backed).

use crate::error::ToolError;
use async_trait::async_trait;

use crate::tmux::capture::run_container_capture_pane;
use crate::tmux::pane::ensure_container_tmux_pane;
use crate::tmux::prompt::ensure_container_tmux_prompt_setup;
use crate::tmux::run::run_container_tmux_process;
use crate::tmux::send_keys::send_container_tmux_keys;
use crate::tools::execution::contracts::{CommandBackend, ExecutionBackendOps};
use crate::tools::execution::file_io::{
    read_file_via_command_backend, write_file_via_command_backend,
};
use crate::tools::execution::process::{
    run_container_sh_process, run_container_tmux_sh_process, run_with_wait,
};
use crate::tools::execution::types::{
    CapturePaneOptions, ContainerContext, ContainerEngineKind, ContainerTmuxContext, ExecOutput,
    SendKeysOptions, ShellWait, TmuxAttachInfo, TmuxAttachTarget, TMUX_WINDOW_NAME,
};

impl ContainerTmuxContext {
    pub(in crate::tools::execution) async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        // Ensure pane exists and prompt bootstrap is active before dispatching.
        let pane_id = self.ensure_prompt_ready().await?;
        run_container_tmux_process(self, &pane_id, command, stdin, wait).await
    }

    pub(in crate::tools::execution) async fn ensure_prompt_ready(
        &self,
    ) -> Result<String, ToolError> {
        // Fast-path: configured pane still exists.
        let configured_pane = self.configured_tmux_pane.lock().await.clone();
        if let Some(pane_id) = configured_pane {
            if self.tmux_pane_exists(&pane_id).await? {
                return Ok(pane_id);
            }
        }

        // Slow-path: ensure pane and initialize prompt markers for new panes.
        let ensured = ensure_container_tmux_pane(self, &self.tmux_session).await?;
        let mut configured = self.configured_tmux_pane.lock().await;
        if ensured.created {
            ensure_container_tmux_prompt_setup(self, &ensured.pane_id).await?;
        }
        if configured.as_deref() != Some(ensured.pane_id.as_str()) {
            *configured = Some(ensured.pane_id.clone());
        }
        Ok(ensured.pane_id)
    }

    async fn tmux_pane_exists(&self, pane_id: &str) -> Result<bool, ToolError> {
        // Probe pane IDs inside container tmux namespace.
        let pane_q = crate::tools::execution::process::shell_quote(pane_id);
        let probe =
            format!("tmux list-panes -a -F '#{{pane_id}}' | grep -Fx -- {pane_q} >/dev/null 2>&1");
        let output = run_container_tmux_sh_process(self, &probe, None).await?;
        Ok(output.exit_code == 0)
    }
}

#[async_trait]
impl CommandBackend for ContainerContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        if matches!(wait, ShellWait::NoWait) {
            // Asynchronous dispatch requires tmux-backed context.
            return Err(ToolError::ExecutionFailed(
                "run_shell wait=false requires a tmux-backed execution target".into(),
            ));
        }
        run_with_wait(
            run_container_sh_process(self, command, stdin),
            wait,
            "timed out waiting for container command completion",
        )
        .await
    }
}

#[async_trait]
impl CommandBackend for ContainerTmuxContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        ContainerTmuxContext::run_command(self, command, stdin, wait).await
    }
}

#[async_trait]
impl ExecutionBackendOps for ContainerContext {
    fn summary(&self) -> String {
        format!(
            "container:{} (via {}{})",
            self.container,
            self.engine.command,
            if self.engine.kind == ContainerEngineKind::Podman {
                ", podman-compatible"
            } else {
                ""
            }
        )
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        None
    }

    fn startup_existing_tmux_pane(&self) -> Option<String> {
        None
    }

    fn capture_pane_available(&self) -> bool {
        false
    }

    async fn capture_pane(&self, _options: CapturePaneOptions) -> Result<String, ToolError> {
        // Raw container backend has no persistent pane to capture.
        Err(ToolError::ExecutionFailed(
            "capture-pane is unavailable for container execution targets".into(),
        ))
    }

    async fn send_keys(&self, _options: SendKeysOptions) -> Result<String, ToolError> {
        // Raw container backend has no persistent pane for key injection.
        Err(ToolError::ExecutionFailed(
            "send-keys is unavailable for container execution targets".into(),
        ))
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

#[async_trait]
impl ExecutionBackendOps for ContainerTmuxContext {
    fn summary(&self) -> String {
        format!(
            "container:{} (tmux:{}) (via {}{})",
            self.container,
            self.tmux_session,
            self.engine.command,
            if self.engine.kind == ContainerEngineKind::Podman {
                ", podman-compatible"
            } else {
                ""
            }
        )
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        Some(TmuxAttachInfo {
            session: self.tmux_session.clone(),
            window: TMUX_WINDOW_NAME,
            target: TmuxAttachTarget::Container {
                engine: self.engine.command.to_string(),
                container: self.container.clone(),
            },
        })
    }

    fn startup_existing_tmux_pane(&self) -> Option<String> {
        self.startup_existing_tmux_pane.clone()
    }

    fn capture_pane_available(&self) -> bool {
        true
    }

    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        // Managed tmux backend defaults to its configured shared pane.
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready().await?
        };
        run_container_capture_pane(self, &pane_target, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
        // Managed tmux backend defaults to its configured shared pane.
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready().await?
        };
        send_container_tmux_keys(self, &pane_target, &options).await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn container_backend_rejects_no_wait_without_tmux() {
        // No-wait mode should be rejected when backend lacks tmux support.
        let ctx = ContainerContext {
            engine: crate::tools::execution::types::ContainerEngine {
                command: "docker",
                kind: ContainerEngineKind::Docker,
            },
            container: "demo".to_string(),
        };

        match ctx.run_command("echo hi", None, ShellWait::NoWait).await {
            Ok(_) => panic!("no-wait should be rejected"),
            Err(err) => assert!(err
                .to_string()
                .contains("requires a tmux-backed execution target")),
        }
    }
}
