//! Container backend implementations (raw and tmux-backed).

use crate::error::ToolError;
use async_trait::async_trait;

use crate::tmux::capture::run_container_capture_pane;
use crate::tmux::management::{
    canonical_session_name, create_managed_pane_script, create_managed_session_script,
    kill_managed_pane_script, kill_managed_session_script, parse_created_pane,
    parse_created_session, parse_killed_pane, parse_resolved_target, resolve_managed_target_script,
};
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
    CapturePaneOptions, ContainerContext, ContainerEngineKind, ContainerTmuxContext,
    CreatedTmuxPane, CreatedTmuxSession, ExecOutput, ResolvedTmuxTarget, SendKeysOptions,
    ShellWait, TmuxAttachInfo, TmuxAttachTarget, TmuxTargetSelector, TMUX_PANE_TITLE,
    TMUX_WINDOW_NAME,
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
        let ensured =
            ensure_container_tmux_pane(self, &self.tmux_session, &self.owner_prefix).await?;
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

    async fn resolve_target(
        &self,
        selector: TmuxTargetSelector,
        ensure_default_shared: bool,
    ) -> Result<ResolvedTmuxTarget, ToolError> {
        let wants_default_shared = selector.target.is_none()
            && selector
                .session
                .as_deref()
                .is_none_or(|session| session.trim() == self.tmux_session)
            && selector
                .pane
                .as_deref()
                .is_none_or(|pane| pane.trim() == TMUX_PANE_TITLE);
        if ensure_default_shared && wants_default_shared {
            let pane_id = self.ensure_prompt_ready().await?;
            return Ok(ResolvedTmuxTarget {
                session: self.tmux_session.clone(),
                pane_id,
                pane_title: TMUX_PANE_TITLE.to_string(),
                is_default_shared: true,
            });
        }

        let script =
            resolve_managed_target_script(&self.owner_prefix, &self.tmux_session, &selector)?;
        let output = run_container_tmux_sh_process(self, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to resolve managed tmux target".into(),
        )?;
        parse_resolved_target(&output.stdout)
            .ok_or_else(|| ToolError::ExecutionFailed("failed to parse managed tmux target".into()))
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

    async fn run_shell_command_targeted(
        &self,
        _command: &str,
        _wait: ShellWait,
        _target: ResolvedTmuxTarget,
    ) -> Result<ExecOutput, ToolError> {
        Err(ToolError::ExecutionFailed(
            "targeted tmux execution requires --tmux".into(),
        ))
    }

    async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        read_file_via_command_backend(self, path).await
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        write_file_via_command_backend(self, path, content).await
    }

    fn tmux_management_available(&self) -> bool {
        false
    }

    async fn resolve_tmux_target(
        &self,
        _selector: TmuxTargetSelector,
        _ensure_default_shared: bool,
    ) -> Result<ResolvedTmuxTarget, ToolError> {
        Err(ToolError::ExecutionFailed(
            "managed tmux targeting requires --tmux".into(),
        ))
    }

    async fn create_tmux_session(&self, _session: String) -> Result<CreatedTmuxSession, ToolError> {
        Err(ToolError::ExecutionFailed(
            "tmux session management requires --tmux".into(),
        ))
    }

    async fn kill_tmux_session(&self, _session: String) -> Result<String, ToolError> {
        Err(ToolError::ExecutionFailed(
            "tmux session management requires --tmux".into(),
        ))
    }

    async fn create_tmux_pane(
        &self,
        _session: Option<String>,
        _pane: String,
    ) -> Result<CreatedTmuxPane, ToolError> {
        Err(ToolError::ExecutionFailed(
            "tmux pane management requires --tmux".into(),
        ))
    }

    async fn kill_tmux_pane(
        &self,
        _session: Option<String>,
        _pane: String,
    ) -> Result<String, ToolError> {
        Err(ToolError::ExecutionFailed(
            "tmux pane management requires --tmux".into(),
        ))
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
        let resolved = self
            .resolve_target(
                TmuxTargetSelector {
                    target: options.target.clone(),
                    session: options.session.clone(),
                    pane: options.pane.clone(),
                },
                true,
            )
            .await?;
        run_container_capture_pane(self, &resolved.pane_id, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
        let resolved = self
            .resolve_target(
                TmuxTargetSelector {
                    target: options.target.clone(),
                    session: options.session.clone(),
                    pane: options.pane.clone(),
                },
                true,
            )
            .await?;
        send_container_tmux_keys(self, &resolved.pane_id, &options).await?;
        Ok(format!(
            "sent keys to tmux pane {} ({})",
            resolved.pane_id, resolved.session
        ))
    }

    async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        self.run_command(command, None, wait).await
    }

    async fn run_shell_command_targeted(
        &self,
        command: &str,
        wait: ShellWait,
        target: ResolvedTmuxTarget,
    ) -> Result<ExecOutput, ToolError> {
        run_container_tmux_process(self, &target.pane_id, command, None, wait).await
    }

    async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        read_file_via_command_backend(self, path).await
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        write_file_via_command_backend(self, path, content).await
    }

    fn tmux_management_available(&self) -> bool {
        true
    }

    async fn resolve_tmux_target(
        &self,
        selector: TmuxTargetSelector,
        ensure_default_shared: bool,
    ) -> Result<ResolvedTmuxTarget, ToolError> {
        self.resolve_target(selector, ensure_default_shared).await
    }

    async fn create_tmux_session(&self, session: String) -> Result<CreatedTmuxSession, ToolError> {
        let session =
            canonical_session_name(&self.owner_prefix, &self.tmux_session, Some(&session))?;
        let script = create_managed_session_script(&self.owner_prefix, &session, self.max_sessions);
        let output = run_container_tmux_sh_process(self, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to create managed tmux session".into(),
        )?;
        let created = parse_created_session(&output.stdout).ok_or_else(|| {
            ToolError::ExecutionFailed("failed to parse created tmux session".into())
        })?;
        if created.created {
            ensure_container_tmux_prompt_setup(self, &created.pane_id).await?;
        }
        Ok(created)
    }

    async fn kill_tmux_session(&self, session: String) -> Result<String, ToolError> {
        let script = kill_managed_session_script(&self.owner_prefix, &self.tmux_session, &session)?;
        let output = run_container_tmux_sh_process(self, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to kill managed tmux session".into(),
        )?;
        let killed = output.stdout.trim();
        if killed.is_empty() {
            return Err(ToolError::ExecutionFailed(
                "failed to parse killed tmux session".into(),
            ));
        }
        Ok(killed.to_string())
    }

    async fn create_tmux_pane(
        &self,
        session: Option<String>,
        pane: String,
    ) -> Result<CreatedTmuxPane, ToolError> {
        let script = create_managed_pane_script(
            &self.owner_prefix,
            &self.tmux_session,
            session.as_deref(),
            &pane,
            self.max_panes,
        )?;
        let output = run_container_tmux_sh_process(self, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to create managed tmux pane".into(),
        )?;
        let created = parse_created_pane(&output.stdout).ok_or_else(|| {
            ToolError::ExecutionFailed("failed to parse created tmux pane".into())
        })?;
        if created.created {
            ensure_container_tmux_prompt_setup(self, &created.pane_id).await?;
        }
        Ok(created)
    }

    async fn kill_tmux_pane(
        &self,
        session: Option<String>,
        pane: String,
    ) -> Result<String, ToolError> {
        let script = kill_managed_pane_script(
            &self.owner_prefix,
            &self.tmux_session,
            session.as_deref(),
            &pane,
        )?;
        let output = run_container_tmux_sh_process(self, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to kill managed tmux pane".into(),
        )?;
        let (session, pane_id) = parse_killed_pane(&output.stdout)
            .ok_or_else(|| ToolError::ExecutionFailed("failed to parse killed tmux pane".into()))?;
        Ok(format!("killed pane {pane_id} in session {session}"))
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
