//! Shared execution backends for tools.
//!
//! The `run_shell`, `read_file`, `write_file`, and tmux `capture-pane` support
//! can run against:
//! - the local machine (default)
//! - a running container (`docker exec` / `podman exec`)
//! - a remote host over SSH with a persistent master connection

mod backend;
mod contracts;
mod file_io;
pub(crate) mod process;
pub(crate) mod types;

use crate::error::ToolError;
use backend::local::ensure_not_in_managed_local_tmux_pane;
use backend::ssh::{
    build_ssh_control_path, close_ssh_control_connection, default_tmux_session_name_for_agent,
};
use contracts::ExecutionBackendOps;
use process::{
    detect_container_engine, ensure_success, run_container_tmux_sh_process, run_process,
    run_sh_process, run_ssh_raw_process, shell_quote,
};
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use crate::tmux::pane::{ensure_container_tmux_pane, ensure_local_tmux_pane, ensure_tmux_pane};
use crate::tmux::prompt::{
    ensure_container_tmux_prompt_setup, ensure_local_tmux_prompt_setup, ensure_tmux_prompt_setup,
};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
#[cfg(test)]
use types::ContainerEngine;
#[cfg(test)]
use types::ContainerEngineKind;
use types::{
    ContainerContext, ContainerTmuxContext, ExecOutput, LocalBackend, LocalTmuxContext, SshContext,
    TMUX_WINDOW_NAME,
};

pub use types::{CapturePaneOptions, SendKeysOptions, ShellWait, TmuxAttachInfo, TmuxAttachTarget};

/// Runtime-execution backend shared across tool instances.
#[derive(Clone)]
pub struct ExecutionContext {
    inner: Arc<dyn ExecutionBackendOps>,
}

impl ExecutionContext {
    /// Build a local execution context.
    pub fn local() -> Self {
        Self {
            inner: Arc::new(LocalBackend),
        }
    }

    /// Build a local tmux-backed execution context.
    ///
    /// This creates (or reuses) a persistent local tmux session so commands can
    /// be dispatched and polled similarly to SSH+tmux mode.
    pub async fn local_tmux(
        requested_tmux_session: Option<String>,
        agent_name: &str,
    ) -> Result<Self, ToolError> {
        if requested_tmux_session
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(ToolError::ExecutionFailed(
                "tmux session name cannot be empty".into(),
            ));
        }

        let probe = run_sh_process("sh", "command -v tmux >/dev/null 2>&1", None).await?;
        if probe.exit_code != 0 {
            return Err(ToolError::ExecutionFailed(
                "local machine does not have tmux installed, but --tmux was provided".into(),
            ));
        }

        let tmux_session = requested_tmux_session
            .unwrap_or_else(|| default_tmux_session_name_for_agent(agent_name));
        ensure_not_in_managed_local_tmux_pane().await?;
        let ensured = ensure_local_tmux_pane(&tmux_session).await?;
        if ensured.created {
            ensure_local_tmux_prompt_setup(&ensured.pane_id).await?;
        }
        let startup_existing_tmux_pane = (!ensured.created).then(|| ensured.pane_id.clone());

        Ok(Self {
            inner: Arc::new(LocalTmuxContext {
                tmux_session,
                configured_tmux_pane: Mutex::new(Some(ensured.pane_id)),
                startup_existing_tmux_pane,
            }),
        })
    }

    /// Build a container execution context.
    pub async fn container(container: impl Into<String>) -> Result<Self, ToolError> {
        let container = container.into();
        if container.trim().is_empty() {
            return Err(ToolError::ExecutionFailed(
                "container id/name cannot be empty".into(),
            ));
        }

        let engine = detect_container_engine().await?;
        Ok(Self {
            inner: Arc::new(ContainerContext { engine, container }),
        })
    }

    /// Build a container execution context backed by a persistent tmux session.
    pub async fn container_tmux(
        container: impl Into<String>,
        requested_tmux_session: Option<String>,
        agent_name: &str,
    ) -> Result<Self, ToolError> {
        let container = container.into();
        if container.trim().is_empty() {
            return Err(ToolError::ExecutionFailed(
                "container id/name cannot be empty".into(),
            ));
        }
        if requested_tmux_session
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(ToolError::ExecutionFailed(
                "tmux session name cannot be empty".into(),
            ));
        }

        let engine = detect_container_engine().await?;
        let default_tmux_session = default_tmux_session_name_for_agent(agent_name);
        let context = ContainerTmuxContext {
            engine,
            container,
            tmux_session: requested_tmux_session.unwrap_or(default_tmux_session),
            configured_tmux_pane: Mutex::new(None),
            startup_existing_tmux_pane: None,
        };

        let probe =
            run_container_tmux_sh_process(&context, "command -v tmux >/dev/null 2>&1", None)
                .await?;
        if probe.exit_code != 0 {
            return Err(ToolError::ExecutionFailed(format!(
                "container {} does not have tmux installed, but --tmux was provided",
                context.container
            )));
        }
        let ensured = ensure_container_tmux_pane(&context, &context.tmux_session).await?;
        if ensured.created {
            ensure_container_tmux_prompt_setup(&context, &ensured.pane_id).await?;
        }
        let startup_existing_tmux_pane = (!ensured.created).then(|| ensured.pane_id.clone());
        {
            let mut configured = context.configured_tmux_pane.lock().await;
            *configured = Some(ensured.pane_id);
        }
        let mut context = context;
        context.startup_existing_tmux_pane = startup_existing_tmux_pane;

        Ok(Self {
            inner: Arc::new(context),
        })
    }

    /// Build an SSH execution context with a persistent master connection.
    ///
    /// If tmux exists on the remote host, this creates a background tmux
    /// session so the operator can reconnect and inspect state later.
    pub async fn ssh(
        target: impl Into<String>,
        requested_tmux_session: Option<String>,
        agent_name: &str,
    ) -> Result<Self, ToolError> {
        let target = target.into();
        if target.trim().is_empty() {
            return Err(ToolError::ExecutionFailed(
                "ssh target cannot be empty".into(),
            ));
        }
        if requested_tmux_session
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(ToolError::ExecutionFailed(
                "tmux session name cannot be empty".into(),
            ));
        }

        let control_path = build_ssh_control_path(&target);
        let open_result = run_process(
            "ssh",
            &[
                "-MNf".into(),
                "-o".into(),
                "ControlMaster=yes".into(),
                "-o".into(),
                "ControlPersist=yes".into(),
                "-o".into(),
                format!("ControlPath={}", control_path.display()),
                target.clone(),
            ],
            None,
        )
        .await?;
        ensure_success(
            open_result,
            "failed to open persistent ssh connection".to_string(),
        )?;

        let tmux_session_result: Result<Option<String>, ToolError> = async {
            let tmux_probe = run_ssh_raw_process(
                &target,
                &control_path,
                "command -v tmux >/dev/null 2>&1",
                None,
            )
            .await?;
            if tmux_probe.exit_code == 0 {
                let session_name = requested_tmux_session
                    .unwrap_or_else(|| default_tmux_session_name_for_agent(agent_name));
                let session_q = shell_quote(&session_name);
                let window_q = shell_quote(TMUX_WINDOW_NAME);
                let script = format!(
                    "tmux has-session -t {session_q} 2>/dev/null || tmux new-session -d -s {session_q} -n {window_q}"
                );
                let tmux_result =
                    run_ssh_raw_process(&target, &control_path, &script, None).await?;
                ensure_success(
                    tmux_result,
                    format!("failed to create remote tmux session {session_name}"),
                )?;
                Ok(Some(session_name))
            } else if requested_tmux_session.is_some() {
                Err(ToolError::ExecutionFailed(
                    "remote host does not have tmux installed, but --tmux was provided".into(),
                ))
            } else {
                Ok(None)
            }
        }
        .await;

        let tmux_session = match tmux_session_result {
            Ok(name) => name,
            Err(err) => {
                close_ssh_control_connection(&target, &control_path);
                return Err(err);
            }
        };

        let (configured_tmux_pane, startup_existing_tmux_pane) = if let Some(session) =
            tmux_session.as_deref()
        {
            match ensure_tmux_pane(&target, &control_path, session).await {
                Ok(ensured) => {
                    if ensured.created {
                        if let Err(err) =
                            ensure_tmux_prompt_setup(&target, &control_path, &ensured.pane_id).await
                        {
                            close_ssh_control_connection(&target, &control_path);
                            return Err(err);
                        }
                    }
                    let startup_existing = (!ensured.created).then(|| ensured.pane_id.clone());
                    (Some(ensured.pane_id), startup_existing)
                }
                Err(err) => {
                    close_ssh_control_connection(&target, &control_path);
                    return Err(err);
                }
            }
        } else {
            (None, None)
        };

        Ok(Self {
            inner: Arc::new(SshContext {
                target,
                control_path,
                tmux_session,
                configured_tmux_pane: Mutex::new(configured_tmux_pane),
                startup_existing_tmux_pane,
            }),
        })
    }

    /// Human-readable execution target summary for UI/status output.
    pub fn summary(&self) -> String {
        self.inner.summary()
    }

    /// Return tmux attach metadata when this context is backed by a managed
    /// tmux session.
    pub fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        self.inner.tmux_attach_info()
    }

    /// Capture the startup pane when this run attached to a pre-existing managed
    /// tmux pane. Returns `Ok(None)` when no existing pane was reused.
    pub async fn capture_startup_existing_tmux_pane(&self) -> Result<Option<String>, ToolError> {
        let Some(pane_target) = self.inner.startup_existing_tmux_pane() else {
            return Ok(None);
        };
        self.capture_pane(CapturePaneOptions {
            target: Some(pane_target),
            ..CapturePaneOptions::default()
        })
        .await
        .map(Some)
    }

    /// Whether tmux pane capture is available for this execution backend.
    pub fn capture_pane_available(&self) -> bool {
        self.inner.capture_pane_available()
    }

    /// Capture pane output using tmux in the active execution backend.
    pub async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        let mut normalized = options;
        if normalized.delay > Duration::ZERO {
            sleep(normalized.delay).await;
            normalized.delay = Duration::ZERO;
        }
        self.inner.capture_pane(normalized).await
    }

    /// Send keys directly to a tmux pane.
    pub async fn send_keys(&self, mut options: SendKeysOptions) -> Result<String, ToolError> {
        if options.keys.is_empty()
            && options
                .literal_text
                .as_deref()
                .map_or(true, |text| text.trim().is_empty())
            && !options.press_enter
        {
            return Err(ToolError::InvalidArguments(
                "send-keys requires at least one of: keys, literal_text, or enter=true".into(),
            ));
        }
        if options.delay > Duration::ZERO {
            sleep(options.delay).await;
            options.delay = Duration::ZERO;
        }
        self.inner.send_keys(options).await
    }

    /// Run a shell command in the selected execution backend.
    pub async fn run_shell_command(
        &self,
        command: &str,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        self.inner.run_shell_command(command, wait).await
    }

    /// Read a text file through the configured backend.
    pub async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        self.inner.read_file(path).await
    }

    /// Write a text file through the configured backend.
    pub async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        self.inner.write_file(path, content).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_tmux_summary_and_capture_availability() {
        let ctx = ExecutionContext {
            inner: Arc::new(LocalTmuxContext {
                tmux_session: "buddy-dev".to_string(),
                configured_tmux_pane: Mutex::new(None),
                startup_existing_tmux_pane: Some("%7".to_string()),
            }),
        };
        assert_eq!(ctx.summary(), "local (tmux:buddy-dev)");
        assert!(ctx.capture_pane_available());
        assert_eq!(
            ctx.inner.startup_existing_tmux_pane(),
            Some("%7".to_string())
        );
        assert_eq!(
            ctx.tmux_attach_info(),
            Some(TmuxAttachInfo {
                session: "buddy-dev".to_string(),
                window: "shared",
                target: TmuxAttachTarget::Local,
            })
        );
    }

    #[test]
    fn container_tmux_summary_and_capture_availability() {
        let ctx = ExecutionContext {
            inner: Arc::new(ContainerTmuxContext {
                engine: ContainerEngine {
                    command: "docker",
                    kind: ContainerEngineKind::Docker,
                },
                container: "devbox".to_string(),
                tmux_session: "buddy-dev".to_string(),
                configured_tmux_pane: Mutex::new(None),
                startup_existing_tmux_pane: None,
            }),
        };
        assert_eq!(
            ctx.summary(),
            "container:devbox (tmux:buddy-dev) (via docker)"
        );
        assert!(ctx.capture_pane_available());
        assert_eq!(
            ctx.tmux_attach_info(),
            Some(TmuxAttachInfo {
                session: "buddy-dev".to_string(),
                window: "shared",
                target: TmuxAttachTarget::Container {
                    engine: "docker".to_string(),
                    container: "devbox".to_string(),
                },
            })
        );
    }

    #[test]
    fn ssh_tmux_summary_and_attach_metadata() {
        let ctx = ExecutionContext {
            inner: Arc::new(SshContext {
                target: "dev@host".to_string(),
                control_path: PathBuf::from("/tmp/buddy-ssh.sock"),
                tmux_session: Some("buddy-4a2f".to_string()),
                configured_tmux_pane: Mutex::new(None),
                startup_existing_tmux_pane: None,
            }),
        };
        assert_eq!(ctx.summary(), "ssh:dev@host (tmux:buddy-4a2f)");
        assert!(ctx.capture_pane_available());
        assert_eq!(
            ctx.tmux_attach_info(),
            Some(TmuxAttachInfo {
                session: "buddy-4a2f".to_string(),
                window: "shared",
                target: TmuxAttachTarget::Ssh {
                    target: "dev@host".to_string(),
                },
            })
        );
    }
}
