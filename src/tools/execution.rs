//! Shared execution backends for tools.
//!
//! The `run_shell`, `read_file`, `write_file`, and tmux `capture-pane` support
//! can run against:
//! - the local machine (default)
//! - a running container (`docker exec` / `podman exec`)
//! - a remote host over SSH with a persistent master connection

use crate::error::ToolError;
use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
#[cfg(test)]
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{sleep, timeout, Duration, Instant};

/// Runtime-execution backend shared across tool instances.
#[derive(Clone)]
pub struct ExecutionContext {
    inner: Arc<dyn ExecutionBackendOps>,
}

/// Internal backend trait used to decouple `ExecutionContext` from concrete
/// local/container/ssh implementations.
#[async_trait]
trait ExecutionBackendOps: Send + Sync {
    fn summary(&self) -> String;
    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo>;
    fn capture_pane_available(&self) -> bool;
    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError>;
    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError>;
    async fn run_shell_command(&self, command: &str, wait: ShellWait)
    -> Result<ExecOutput, ToolError>;
    async fn read_file(&self, path: &str) -> Result<String, ToolError>;
    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError>;
}

/// Shared contract for backends that can execute shell snippets.
#[async_trait]
trait CommandBackend: Send + Sync {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError>;
}

/// Structured process output for shell-style commands.
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Waiting behavior for `run_shell` execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellWait {
    /// Wait for command completion with no explicit timeout.
    Wait,
    /// Wait for command completion, but fail if it exceeds this timeout.
    WaitWithTimeout(Duration),
    /// Do not wait for completion; fire command and return immediately.
    NoWait,
}

/// Options for tmux `capture-pane` operations.
///
/// These options are intentionally close to tmux's native flags so tool-level
/// callers can expose common capture behaviors without coupling to shell text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturePaneOptions {
    pub target: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub join_wrapped_lines: bool,
    pub preserve_trailing_spaces: bool,
    pub include_escape_sequences: bool,
    pub escape_non_printable: bool,
    pub include_alternate_screen: bool,
    pub delay: Duration,
}

impl Default for CapturePaneOptions {
    fn default() -> Self {
        Self {
            target: None,
            start: None,
            end: None,
            join_wrapped_lines: true,
            preserve_trailing_spaces: false,
            include_escape_sequences: false,
            escape_non_printable: false,
            include_alternate_screen: false,
            delay: Duration::ZERO,
        }
    }
}

/// Options for tmux key injection against a pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SendKeysOptions {
    pub target: Option<String>,
    pub keys: Vec<String>,
    pub literal_text: Option<String>,
    pub press_enter: bool,
    pub delay: Duration,
}

impl Default for SendKeysOptions {
    fn default() -> Self {
        Self {
            target: None,
            keys: Vec::new(),
            literal_text: None,
            press_enter: false,
            delay: Duration::ZERO,
        }
    }
}

/// Attach metadata for tmux-backed execution targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TmuxAttachInfo {
    pub session: String,
    pub window: &'static str,
    pub target: TmuxAttachTarget,
}

/// Concrete execution target for tmux attach instructions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TmuxAttachTarget {
    Local,
    Ssh { target: String },
    Container { engine: String, container: String },
}

struct ContainerContext {
    engine: ContainerEngine,
    container: String,
}

struct LocalBackend;

struct ContainerTmuxContext {
    engine: ContainerEngine,
    container: String,
    tmux_session: String,
    configured_tmux_pane: Mutex<Option<String>>,
}

struct ContainerEngine {
    command: &'static str,
    kind: ContainerEngineKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContainerEngineKind {
    Docker,
    Podman,
}

const TMUX_WINDOW_NAME: &str = "buddy-shared";
const LOCAL_TMUX_SESSION_SEED: &str = "local";

struct LocalTmuxContext {
    tmux_session: String,
    configured_tmux_pane: Mutex<Option<String>>,
}

struct SshContext {
    target: String,
    control_path: PathBuf,
    tmux_session: Option<String>,
    configured_tmux_pane: Mutex<Option<String>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EnsuredTmuxPane {
    pane_id: String,
    created: bool,
}

impl SshContext {
    /// Run a command on the remote host, forcing tmux execution when a tmux
    /// session is configured for this connection.
    async fn run_command(
        &self,
        remote_command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        if let Some(session) = &self.tmux_session {
            let pane_id = self.ensure_prompt_ready(session).await?;
            run_ssh_tmux_process(
                &self.target,
                &self.control_path,
                &pane_id,
                remote_command,
                stdin,
                wait,
            )
            .await
        } else {
            if matches!(wait, ShellWait::NoWait) {
                return Err(ToolError::ExecutionFailed(
                    "run_shell wait=false requires a tmux-backed execution target".into(),
                ));
            }
            run_with_wait(
                run_ssh_raw_process(&self.target, &self.control_path, remote_command, stdin),
                wait,
                "timed out waiting for ssh command completion",
            )
            .await
        }
    }

    async fn ensure_prompt_ready(&self, tmux_session: &str) -> Result<String, ToolError> {
        let ensured = ensure_tmux_pane(&self.target, &self.control_path, tmux_session).await?;
        let mut configured = self.configured_tmux_pane.lock().await;
        if ensured.created {
            ensure_tmux_prompt_setup(&self.target, &self.control_path, &ensured.pane_id).await?;
        }
        if configured.as_deref() != Some(ensured.pane_id.as_str()) {
            *configured = Some(ensured.pane_id.clone());
        }
        Ok(ensured.pane_id)
    }
}

impl LocalTmuxContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        let pane_id = self.ensure_prompt_ready().await?;
        run_local_tmux_process(&pane_id, command, stdin, wait).await
    }

    async fn ensure_prompt_ready(&self) -> Result<String, ToolError> {
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

impl ContainerTmuxContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        let pane_id = self.ensure_prompt_ready().await?;
        run_container_tmux_process(self, &pane_id, command, stdin, wait).await
    }

    async fn ensure_prompt_ready(&self) -> Result<String, ToolError> {
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
impl CommandBackend for ContainerContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        if matches!(wait, ShellWait::NoWait) {
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
impl CommandBackend for SshContext {
    async fn run_command(
        &self,
        command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        SshContext::run_command(self, command, stdin, wait).await
    }
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
    pub async fn local_tmux(requested_tmux_session: Option<String>) -> Result<Self, ToolError> {
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

        let tmux_session = requested_tmux_session.unwrap_or_else(default_local_tmux_session_name);
        ensure_not_in_managed_local_tmux_pane().await?;
        let ensured = ensure_local_tmux_pane(&tmux_session).await?;
        if ensured.created {
            ensure_local_tmux_prompt_setup(&ensured.pane_id).await?;
        }

        Ok(Self {
            inner: Arc::new(LocalTmuxContext {
                tmux_session,
                configured_tmux_pane: Mutex::new(Some(ensured.pane_id)),
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
            inner: Arc::new(ContainerContext {
                engine,
                container,
            }),
        })
    }

    /// Build a container execution context backed by a persistent tmux session.
    pub async fn container_tmux(
        container: impl Into<String>,
        requested_tmux_session: Option<String>,
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
        let default_tmux_session = default_tmux_session_name(&container);
        let context = ContainerTmuxContext {
            engine,
            container,
            tmux_session: requested_tmux_session.unwrap_or(default_tmux_session),
            configured_tmux_pane: Mutex::new(None),
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
        {
            let mut configured = context.configured_tmux_pane.lock().await;
            *configured = Some(ensured.pane_id);
        }

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
                    .unwrap_or_else(|| default_tmux_session_name(&target));
                let session_q = shell_quote(&session_name);
                let script = format!(
                    "tmux has-session -t {session_q} 2>/dev/null || tmux new-session -d -s {session_q}"
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

        let configured_tmux_pane = if let Some(session) = tmux_session.as_deref() {
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
                    Some(ensured.pane_id)
                }
                Err(err) => {
                    close_ssh_control_connection(&target, &control_path);
                    return Err(err);
                }
            }
        } else {
            None
        };

        Ok(Self {
            inner: Arc::new(SshContext {
                target,
                control_path,
                tmux_session,
                configured_tmux_pane: Mutex::new(configured_tmux_pane),
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

#[async_trait]
impl ExecutionBackendOps for LocalBackend {
    fn summary(&self) -> String {
        "local".to_string()
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
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

    fn capture_pane_available(&self) -> bool {
        false
    }

    async fn capture_pane(&self, _options: CapturePaneOptions) -> Result<String, ToolError> {
        Err(ToolError::ExecutionFailed(
            "capture-pane is unavailable for container execution targets".into(),
        ))
    }

    async fn send_keys(&self, _options: SendKeysOptions) -> Result<String, ToolError> {
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

    fn capture_pane_available(&self) -> bool {
        true
    }

    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready().await?
        };
        run_container_capture_pane(self, &pane_target, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
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

#[async_trait]
impl ExecutionBackendOps for SshContext {
    fn summary(&self) -> String {
        let mut base = format!("ssh:{}", self.target);
        if let Some(name) = &self.tmux_session {
            base.push_str(&format!(" (tmux:{name})"));
        }
        base
    }

    fn tmux_attach_info(&self) -> Option<TmuxAttachInfo> {
        self.tmux_session.as_ref().map(|session| TmuxAttachInfo {
            session: session.clone(),
            window: TMUX_WINDOW_NAME,
            target: TmuxAttachTarget::Ssh {
                target: self.target.clone(),
            },
        })
    }

    fn capture_pane_available(&self) -> bool {
        self.tmux_session.is_some()
    }

    async fn capture_pane(&self, options: CapturePaneOptions) -> Result<String, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "capture-pane is unavailable: no tmux session for this ssh target".into(),
            )
        })?;
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready(tmux_session).await?
        };
        run_remote_capture_pane(&self.target, &self.control_path, &pane_target, &options).await
    }

    async fn send_keys(&self, options: SendKeysOptions) -> Result<String, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "send-keys is unavailable: no tmux session for this ssh target".into(),
            )
        })?;
        let pane_target = if let Some(target) = options.target.as_deref() {
            target.to_string()
        } else {
            self.ensure_prompt_ready(tmux_session).await?
        };
        send_remote_tmux_keys(&self.target, &self.control_path, &pane_target, &options).await?;
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

async fn read_file_via_command_backend(
    backend: &(impl CommandBackend + ?Sized),
    path: &str,
) -> Result<String, ToolError> {
    let script = format!("cat -- {}", shell_quote(path));
    let output = backend.run_command(&script, None, ShellWait::Wait).await?;
    ensure_success(output, path.to_string()).map(|out| out.stdout)
}

async fn write_file_via_command_backend(
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

impl Drop for SshContext {
    fn drop(&mut self) {
        // Best-effort connection cleanup; failures are non-fatal.
        close_ssh_control_connection(&self.target, &self.control_path);
    }
}

#[cfg(test)]
type SshCloseHook = Box<dyn Fn(&str, &Path) + Send + Sync + 'static>;

#[cfg(test)]
fn ssh_close_hook_slot() -> &'static StdMutex<Option<SshCloseHook>> {
    static SLOT: OnceLock<StdMutex<Option<SshCloseHook>>> = OnceLock::new();
    SLOT.get_or_init(|| StdMutex::new(None))
}

#[cfg(test)]
fn set_ssh_close_hook_for_tests(hook: Option<SshCloseHook>) {
    *ssh_close_hook_slot().lock().expect("ssh close hook lock") = hook;
}

fn close_ssh_control_connection(target: &str, control_path: &Path) {
    #[cfg(test)]
    {
        if let Some(hook) = ssh_close_hook_slot()
            .lock()
            .expect("ssh close hook lock")
            .as_ref()
        {
            hook(target, control_path);
            return;
        }
    }

    let _ = std::process::Command::new("ssh")
        .arg("-S")
        .arg(control_path)
        .arg("-O")
        .arg("exit")
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = std::fs::remove_file(control_path);
}

fn build_ssh_control_path(target: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let hash = hasher.finish();
    std::env::temp_dir().join(format!("buddy-ssh-{hash:x}.sock"))
}

fn default_tmux_session_name(target: &str) -> String {
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    let hash = hasher.finish();
    format!("buddy-{:04x}", (hash & 0xffff) as u16)
}

fn default_local_tmux_session_name() -> String {
    default_tmux_session_name(LOCAL_TMUX_SESSION_SEED)
}

async fn ensure_not_in_managed_local_tmux_pane() -> Result<(), ToolError> {
    let Some(current_pane) = local_tmux_pane_target() else {
        return Ok(());
    };
    let pane_q = shell_quote(&current_pane);
    let inspect = format!("tmux display-message -p -t {pane_q} '#{{window_name}}'");
    let output = run_sh_process("sh", &inspect, None).await?;
    let output = ensure_success(output, "failed to inspect current tmux pane".into())?;
    if is_managed_tmux_window_name(output.stdout.trim()) {
        return Err(ToolError::ExecutionFailed(
            "buddy should be run from a different terminal when --tmux is enabled (current pane is buddy-shared)".into(),
        ));
    }
    Ok(())
}

fn is_managed_tmux_window_name(window_name: &str) -> bool {
    window_name.trim() == TMUX_WINDOW_NAME
}

fn local_tmux_pane_target() -> Option<String> {
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

fn local_tmux_allowed() -> bool {
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

fn build_capture_pane_command(target: &str, options: &CapturePaneOptions) -> String {
    let mut cmd = String::from("tmux capture-pane -p");
    if options.join_wrapped_lines {
        cmd.push_str(" -J");
    }
    if options.preserve_trailing_spaces {
        cmd.push_str(" -N");
    }
    if options.include_escape_sequences {
        cmd.push_str(" -e");
    }
    if options.escape_non_printable {
        cmd.push_str(" -C");
    }
    if options.include_alternate_screen {
        cmd.push_str(" -a");
    }
    if let Some(start) = options.start.as_deref() {
        cmd.push_str(" -S ");
        cmd.push_str(&shell_quote(start));
    }
    if let Some(end) = options.end.as_deref() {
        cmd.push_str(" -E ");
        cmd.push_str(&shell_quote(end));
    }
    cmd.push_str(" -t ");
    cmd.push_str(&shell_quote(target));
    cmd
}

fn full_history_capture_options() -> CapturePaneOptions {
    CapturePaneOptions {
        start: Some("-".to_string()),
        end: Some("-".to_string()),
        ..CapturePaneOptions::default()
    }
}

async fn run_local_capture_pane(
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_sh_process("sh", &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output = run_sh_process("sh", &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

async fn run_remote_capture_pane(
    target: &str,
    control_path: &Path,
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_ssh_raw_process(target, control_path, &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output =
                run_ssh_raw_process(target, control_path, &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

async fn run_container_capture_pane(
    ctx: &ContainerTmuxContext,
    pane_target: &str,
    options: &CapturePaneOptions,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_target, options);
    let output = run_container_tmux_sh_process(ctx, &capture_cmd, None).await?;
    match ensure_success(output, "failed to capture tmux pane".into()) {
        Ok(out) => Ok(out.stdout),
        Err(err) if should_fallback_from_alternate_screen(options, &err) => {
            let mut fallback = options.clone();
            fallback.include_alternate_screen = false;
            let fallback_cmd = build_capture_pane_command(pane_target, &fallback);
            let fallback_output = run_container_tmux_sh_process(ctx, &fallback_cmd, None).await?;
            let out = ensure_success(
                fallback_output,
                "failed to capture tmux pane after alternate-screen fallback".into(),
            )?;
            Ok(format!(
                "{}\n\n[notice] alternate screen was not active; captured main pane instead.",
                out.stdout
            ))
        }
        Err(err) => Err(err),
    }
}

fn should_fallback_from_alternate_screen(options: &CapturePaneOptions, err: &ToolError) -> bool {
    options.include_alternate_screen
        && err
            .to_string()
            .to_ascii_lowercase()
            .contains("no alternate screen")
}

async fn run_container_sh_process(
    ctx: &ContainerContext,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_container_sh_process_with(&ctx.engine, &ctx.container, command, stdin).await
}

async fn run_container_tmux_sh_process(
    ctx: &ContainerTmuxContext,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_container_sh_process_with(&ctx.engine, &ctx.container, command, stdin).await
}

async fn run_container_sh_process_with(
    engine: &ContainerEngine,
    container: &str,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    let mut args = vec!["exec".to_string()];
    if stdin.is_some() {
        // `podman` and `docker` both support stdin-interactive exec, but
        // the long flag differs in older environments. We switch explicitly
        // based on detected frontend kind.
        let interactive_flag = match engine.kind {
            ContainerEngineKind::Docker => "-i",
            ContainerEngineKind::Podman => "--interactive",
        };
        args.push(interactive_flag.to_string());
    }
    args.push(container.to_string());
    args.push("sh".into());
    args.push("-lc".into());
    args.push(command.into());
    run_process(engine.command, &args, stdin).await
}

async fn run_sh_process(
    shell: &str,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_process(shell, &["-c".into(), command.into()], stdin).await
}

async fn run_ssh_raw_process(
    target: &str,
    control_path: &Path,
    remote_command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_process(
        "ssh",
        &[
            "-T".into(),
            "-S".into(),
            control_path.display().to_string(),
            "-o".into(),
            "ControlMaster=no".into(),
            target.into(),
            remote_command.into(),
        ],
        stdin,
    )
    .await
}

async fn run_ssh_tmux_process(
    target: &str,
    control_path: &Path,
    pane_id: &str,
    remote_command: &str,
    stdin: Option<&[u8]>,
    wait: ShellWait,
) -> Result<ExecOutput, ToolError> {
    if matches!(wait, ShellWait::NoWait) {
        if stdin.is_some() {
            return Err(ToolError::ExecutionFailed(
                "run_shell wait=false does not support stdin input".into(),
            ));
        }
        send_tmux_line(target, control_path, pane_id, remote_command).await?;
        return Ok(ExecOutput {
            exit_code: 0,
            stdout: format!(
                "command dispatched to tmux pane {pane_id}; still running in background. Use capture-pane (optionally with delay) to poll output."
            ),
            stderr: String::new(),
        });
    }

    // Snapshot full pane content so parsing can compute robust deltas even when
    // tmux history scrolls and total line count remains constant.
    let baseline_capture = capture_tmux_pane(target, control_path, pane_id).await?;
    let start_marker = latest_prompt_marker(&baseline_capture).ok_or_else(|| {
        ToolError::ExecutionFailed(
            "failed to detect baseline tmux prompt marker before command execution".into(),
        )
    })?;

    // For file writes, stage stdin in a temp file and redirect from it.
    let mut staged_workdir = None;
    let mut run_command = remote_command.to_string();
    if let Some(input) = stdin {
        let token = unique_token(target, remote_command);
        let workdir = format!("/tmp/buddy-tmux-{token}");
        let input_file = format!("{workdir}/stdin");
        let workdir_q = shell_quote(&workdir);
        let input_q = shell_quote(&input_file);
        let stage_cmd = format!("mkdir -p {workdir_q}; cat > {input_q}");
        let staged = run_ssh_raw_process(target, control_path, &stage_cmd, Some(input)).await?;
        ensure_success(staged, "failed to stage tmux stdin".into())?;
        run_command = format!("{run_command} < {input_q}");
        staged_workdir = Some(workdir_q);
    }

    // Execute the exact command text in the shared pane (no shell wrapper).
    send_tmux_line(target, control_path, pane_id, &run_command).await?;
    let result = wait_for_tmux_result(
        target,
        control_path,
        pane_id,
        start_marker.command_id,
        &run_command,
        match wait {
            ShellWait::Wait => None,
            ShellWait::WaitWithTimeout(limit) => Some(limit),
            ShellWait::NoWait => None,
        },
    )
    .await;

    if let Some(workdir_q) = staged_workdir {
        let _ =
            run_ssh_raw_process(target, control_path, &format!("rm -rf {workdir_q}"), None).await;
    }

    result
}

async fn run_local_tmux_process(
    pane_id: &str,
    command: &str,
    stdin: Option<&[u8]>,
    wait: ShellWait,
) -> Result<ExecOutput, ToolError> {
    if matches!(wait, ShellWait::NoWait) {
        if stdin.is_some() {
            return Err(ToolError::ExecutionFailed(
                "run_shell wait=false does not support stdin input".into(),
            ));
        }
        send_local_tmux_line(pane_id, command).await?;
        return Ok(ExecOutput {
            exit_code: 0,
            stdout: format!(
                "command dispatched to tmux pane {pane_id}; still running in background. Use capture-pane (optionally with delay) to poll output."
            ),
            stderr: String::new(),
        });
    }

    let baseline_capture = capture_local_tmux_pane(pane_id).await?;
    let start_marker = latest_prompt_marker(&baseline_capture).ok_or_else(|| {
        ToolError::ExecutionFailed(
            "failed to detect baseline tmux prompt marker before command execution".into(),
        )
    })?;

    let mut staged_workdir = None;
    let mut run_command = command.to_string();
    if let Some(input) = stdin {
        let token = unique_token("local", command);
        let workdir = format!("/tmp/buddy-tmux-{token}");
        let input_file = format!("{workdir}/stdin");
        let workdir_q = shell_quote(&workdir);
        let input_q = shell_quote(&input_file);
        let stage_cmd = format!("mkdir -p {workdir_q}; cat > {input_q}");
        let staged = run_sh_process("sh", &stage_cmd, Some(input)).await?;
        ensure_success(staged, "failed to stage tmux stdin".into())?;
        run_command = format!("{run_command} < {input_q}");
        staged_workdir = Some(workdir_q);
    }

    send_local_tmux_line(pane_id, &run_command).await?;
    let result = wait_for_local_tmux_result(
        pane_id,
        start_marker.command_id,
        &run_command,
        match wait {
            ShellWait::Wait => None,
            ShellWait::WaitWithTimeout(limit) => Some(limit),
            ShellWait::NoWait => None,
        },
    )
    .await;

    if let Some(workdir_q) = staged_workdir {
        let _ = run_sh_process("sh", &format!("rm -rf {workdir_q}"), None).await;
    }

    result
}

async fn run_container_tmux_process(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    command: &str,
    stdin: Option<&[u8]>,
    wait: ShellWait,
) -> Result<ExecOutput, ToolError> {
    if matches!(wait, ShellWait::NoWait) {
        if stdin.is_some() {
            return Err(ToolError::ExecutionFailed(
                "run_shell wait=false does not support stdin input".into(),
            ));
        }
        send_container_tmux_line(ctx, pane_id, command).await?;
        return Ok(ExecOutput {
            exit_code: 0,
            stdout: format!(
                "command dispatched to tmux pane {pane_id}; still running in background. Use capture-pane (optionally with delay) to poll output."
            ),
            stderr: String::new(),
        });
    }

    let baseline_capture = capture_container_tmux_pane(ctx, pane_id).await?;
    let start_marker = latest_prompt_marker(&baseline_capture).ok_or_else(|| {
        ToolError::ExecutionFailed(
            "failed to detect baseline tmux prompt marker before command execution".into(),
        )
    })?;

    let mut staged_workdir = None;
    let mut run_command = command.to_string();
    if let Some(input) = stdin {
        let token = unique_token(&ctx.container, command);
        let workdir = format!("/tmp/buddy-tmux-{token}");
        let input_file = format!("{workdir}/stdin");
        let workdir_q = shell_quote(&workdir);
        let input_q = shell_quote(&input_file);
        let stage_cmd = format!("mkdir -p {workdir_q}; cat > {input_q}");
        let staged = run_container_tmux_sh_process(ctx, &stage_cmd, Some(input)).await?;
        ensure_success(staged, "failed to stage tmux stdin".into())?;
        run_command = format!("{run_command} < {input_q}");
        staged_workdir = Some(workdir_q);
    }

    send_container_tmux_line(ctx, pane_id, &run_command).await?;
    let result = wait_for_container_tmux_result(
        ctx,
        pane_id,
        start_marker.command_id,
        &run_command,
        match wait {
            ShellWait::Wait => None,
            ShellWait::WaitWithTimeout(limit) => Some(limit),
            ShellWait::NoWait => None,
        },
    )
    .await;

    if let Some(workdir_q) = staged_workdir {
        let _ = run_container_tmux_sh_process(ctx, &format!("rm -rf {workdir_q}"), None).await;
    }

    result
}

fn ensure_tmux_pane_script(tmux_session: &str) -> String {
    let session_q = shell_quote(tmux_session);
    let window_q = shell_quote(TMUX_WINDOW_NAME);
    format!(
        "set -e\n\
SESSION={session_q}\n\
WINDOW={window_q}\n\
CREATED=0\n\
if tmux has-session -t \"$SESSION\" 2>/dev/null; then\n\
  :\n\
else\n\
  tmux new-session -d -s \"$SESSION\" -n \"$WINDOW\"\n\
  CREATED=1\n\
fi\n\
if ! tmux list-windows -t \"$SESSION\" -F '#{{window_name}}' | grep -Fx -- \"$WINDOW\" >/dev/null 2>&1; then\n\
  tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\"\n\
  CREATED=1\n\
fi\n\
PANE=\"$(tmux list-panes -t \"$SESSION:$WINDOW\" -F '#{{pane_id}}' | head -n1)\"\n\
if [ -z \"$PANE\" ]; then\n\
  echo \"failed to resolve tmux pane target for $SESSION:$WINDOW\" >&2\n\
  exit 1\n\
fi\n\
printf '%s\n%s' \"$PANE\" \"$CREATED\"\n"
    )
}

async fn ensure_tmux_pane(
    target: &str,
    control_path: &Path,
    tmux_session: &str,
) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_ssh_raw_process(target, control_path, &script, None).await?;
    let output = ensure_success(output, "failed to prepare tmux session pane".into())?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

async fn ensure_local_tmux_pane(tmux_session: &str) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_sh_process("sh", &script, None).await?;
    let output = ensure_success(output, "failed to prepare local tmux session pane".into())?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

async fn ensure_container_tmux_pane(
    ctx: &ContainerTmuxContext,
    tmux_session: &str,
) -> Result<EnsuredTmuxPane, ToolError> {
    let script = ensure_tmux_pane_script(tmux_session);
    let output = run_container_tmux_sh_process(ctx, &script, None).await?;
    let output = ensure_success(
        output,
        "failed to prepare container tmux session pane".into(),
    )?;
    parse_ensured_tmux_pane(&output.stdout)
        .ok_or_else(|| ToolError::ExecutionFailed("failed to resolve tmux pane target".into()))
}

fn tmux_prompt_setup_script() -> &'static str {
    "if [ \"${BUDDY_PROMPT_LAYOUT:-}\" != \"v3\" ]; then \
BUDDY_PROMPT_LAYOUT=v3; \
BUDDY_CMD_SEQ=${BUDDY_CMD_SEQ:-0}; \
__buddy_next_id() { BUDDY_CMD_SEQ=$((BUDDY_CMD_SEQ + 1)); BUDDY_CMD_ID=$BUDDY_CMD_SEQ; }; \
__buddy_prompt_id() { printf '%s' \"${BUDDY_CMD_ID:-0}\"; }; \
if [ -n \"${BASH_VERSION:-}\" ]; then \
BUDDY_BASE_PS1=${BUDDY_BASE_PS1:-$PS1}; \
__buddy_precmd() { __buddy_next_id; }; \
case \";${PROMPT_COMMAND:-};\" in \
  *\";__buddy_precmd;\"*) ;; \
  *) PROMPT_COMMAND=\"__buddy_precmd${PROMPT_COMMAND:+;${PROMPT_COMMAND}}\" ;; \
esac; \
PS1='[buddy $(__buddy_prompt_id): \\?] '\"$BUDDY_BASE_PS1\"; \
elif [ -n \"${ZSH_VERSION:-}\" ]; then \
BUDDY_BASE_PROMPT=${BUDDY_BASE_PROMPT:-$PROMPT}; \
__buddy_precmd() { __buddy_next_id; }; \
if (( ${precmd_functions[(Ie)__buddy_precmd]} == 0 )); then \
  precmd_functions=(__buddy_precmd $precmd_functions); \
fi; \
setopt PROMPT_SUBST; \
PROMPT='[buddy $(__buddy_prompt_id): %?] '\"$BUDDY_BASE_PROMPT\"; \
else \
BUDDY_BASE_PS1=${BUDDY_BASE_PS1:-$PS1}; \
PS1='[buddy $(__buddy_next_id): $?] '\"$BUDDY_BASE_PS1\"; \
fi; \
fi"
}

async fn ensure_tmux_prompt_setup(
    target: &str,
    control_path: &Path,
    pane_id: &str,
) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();

    send_tmux_line(target, control_path, pane_id, configure_prompt).await?;
    wait_for_tmux_any_prompt(target, control_path, pane_id).await?;
    send_tmux_line(target, control_path, pane_id, "clear").await?;
    wait_for_tmux_any_prompt(target, control_path, pane_id).await
}

async fn ensure_local_tmux_prompt_setup(pane_id: &str) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();
    send_local_tmux_line(pane_id, configure_prompt).await?;
    wait_for_local_tmux_any_prompt(pane_id).await?;
    send_local_tmux_line(pane_id, "clear").await?;
    wait_for_local_tmux_any_prompt(pane_id).await
}

async fn ensure_container_tmux_prompt_setup(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<(), ToolError> {
    let configure_prompt = tmux_prompt_setup_script();
    send_container_tmux_line(ctx, pane_id, configure_prompt).await?;
    wait_for_container_tmux_any_prompt(ctx, pane_id).await?;
    send_container_tmux_line(ctx, pane_id, "clear").await?;
    wait_for_container_tmux_any_prompt(ctx, pane_id).await
}

fn parse_ensured_tmux_pane(output: &str) -> Option<EnsuredTmuxPane> {
    let mut lines = output.lines();
    let pane_id = lines.next()?.trim();
    let created_raw = lines.next()?.trim();
    if pane_id.is_empty() {
        return None;
    }
    let created = match created_raw {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    Some(EnsuredTmuxPane {
        pane_id: pane_id.to_string(),
        created,
    })
}

async fn send_tmux_line(
    target: &str,
    control_path: &Path,
    pane_id: &str,
    text: &str,
) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);

    let send_text = run_ssh_raw_process(
        target,
        control_path,
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter = run_ssh_raw_process(
        target,
        control_path,
        &format!("tmux send-keys -t {pane_q} Enter"),
        None,
    )
    .await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;

    Ok(())
}

async fn send_local_tmux_line(pane_id: &str, text: &str) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);
    let send_text = run_sh_process(
        "sh",
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter =
        run_sh_process("sh", &format!("tmux send-keys -t {pane_q} Enter"), None).await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;
    Ok(())
}

async fn send_container_tmux_line(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    text: &str,
) -> Result<(), ToolError> {
    let pane_q = shell_quote(pane_id);
    let text_q = shell_quote(text);
    let send_text = run_container_tmux_sh_process(
        ctx,
        &format!("tmux send-keys -l -t {pane_q} {text_q}"),
        None,
    )
    .await?;
    ensure_success(send_text, "failed to send keys to tmux pane".into())?;

    let send_enter =
        run_container_tmux_sh_process(ctx, &format!("tmux send-keys -t {pane_q} Enter"), None)
            .await?;
    ensure_success(send_enter, "failed to send Enter to tmux pane".into())?;
    Ok(())
}

async fn send_local_tmux_keys(target: &str, options: &SendKeysOptions) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(target, text);
            let output = run_sh_process("sh", &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(target, &options.keys);
        let output = run_sh_process("sh", &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(target);
        let output = run_sh_process("sh", &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

async fn send_remote_tmux_keys(
    target: &str,
    control_path: &Path,
    pane_id: &str,
    options: &SendKeysOptions,
) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(pane_id, text);
            let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(pane_id, &options.keys);
        let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(pane_id);
        let output = run_ssh_raw_process(target, control_path, &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

async fn send_container_tmux_keys(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    options: &SendKeysOptions,
) -> Result<(), ToolError> {
    if let Some(text) = options.literal_text.as_deref() {
        if !text.is_empty() {
            let cmd = build_tmux_send_literal_command(pane_id, text);
            let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
            ensure_success(output, "failed to send literal keys to tmux pane".into())?;
        }
    }
    if !options.keys.is_empty() {
        let cmd = build_tmux_send_keys_command(pane_id, &options.keys);
        let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
        ensure_success(output, "failed to send key sequence to tmux pane".into())?;
    }
    if options.press_enter {
        let cmd = build_tmux_send_enter_command(pane_id);
        let output = run_container_tmux_sh_process(ctx, &cmd, None).await?;
        ensure_success(output, "failed to send Enter to tmux pane".into())?;
    }
    Ok(())
}

fn build_tmux_send_literal_command(target: &str, text: &str) -> String {
    let target_q = shell_quote(target);
    let text_q = shell_quote(text);
    format!("tmux send-keys -l -t {target_q} {text_q}")
}

fn build_tmux_send_keys_command(target: &str, keys: &[String]) -> String {
    let target_q = shell_quote(target);
    let keys_q = keys
        .iter()
        .map(|key| shell_quote(key))
        .collect::<Vec<_>>()
        .join(" ");
    format!("tmux send-keys -t {target_q} {keys_q}")
}

fn build_tmux_send_enter_command(target: &str) -> String {
    let target_q = shell_quote(target);
    format!("tmux send-keys -t {target_q} Enter")
}

async fn wait_for_tmux_result(
    target: &str,
    control_path: &Path,
    pane_id: &str,
    start_command_id: u64,
    command: &str,
    timeout_limit: Option<Duration>,
) -> Result<ExecOutput, ToolError> {
    let started_at = Instant::now();
    loop {
        let capture = capture_tmux_pane(target, control_path, pane_id).await?;
        if let Some(parsed) = parse_tmux_capture_output(&capture, start_command_id, command) {
            return parsed;
        }
        if let Some(limit) = timeout_limit {
            if started_at.elapsed() >= limit {
                return Err(ToolError::ExecutionFailed(format!(
                    "timed out waiting for tmux command completion after {}",
                    format_duration(limit)
                )));
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_local_tmux_result(
    pane_id: &str,
    start_command_id: u64,
    command: &str,
    timeout_limit: Option<Duration>,
) -> Result<ExecOutput, ToolError> {
    let started_at = Instant::now();
    loop {
        let capture = capture_local_tmux_pane(pane_id).await?;
        if let Some(parsed) = parse_tmux_capture_output(&capture, start_command_id, command) {
            return parsed;
        }
        if let Some(limit) = timeout_limit {
            if started_at.elapsed() >= limit {
                return Err(ToolError::ExecutionFailed(format!(
                    "timed out waiting for tmux command completion after {}",
                    format_duration(limit)
                )));
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_container_tmux_result(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
    start_command_id: u64,
    command: &str,
    timeout_limit: Option<Duration>,
) -> Result<ExecOutput, ToolError> {
    let started_at = Instant::now();
    loop {
        let capture = capture_container_tmux_pane(ctx, pane_id).await?;
        if let Some(parsed) = parse_tmux_capture_output(&capture, start_command_id, command) {
            return parsed;
        }
        if let Some(limit) = timeout_limit {
            if started_at.elapsed() >= limit {
                return Err(ToolError::ExecutionFailed(format!(
                    "timed out waiting for tmux command completion after {}",
                    format_duration(limit)
                )));
            }
        }
        sleep(Duration::from_millis(50)).await;
    }
}

async fn capture_tmux_pane(
    target: &str,
    control_path: &Path,
    pane_id: &str,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_ssh_raw_process(target, control_path, &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

async fn capture_local_tmux_pane(pane_id: &str) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_sh_process("sh", &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

async fn capture_container_tmux_pane(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<String, ToolError> {
    let capture_cmd = build_capture_pane_command(pane_id, &full_history_capture_options());
    let capture = run_container_tmux_sh_process(ctx, &capture_cmd, None).await?;
    ensure_success(capture, "failed to capture tmux pane".into()).map(|out| out.stdout)
}

async fn wait_for_tmux_any_prompt(
    target: &str,
    control_path: &Path,
    pane_id: &str,
) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_tmux_pane(target, control_path, pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

async fn wait_for_local_tmux_any_prompt(pane_id: &str) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_local_tmux_pane(pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

async fn wait_for_container_tmux_any_prompt(
    ctx: &ContainerTmuxContext,
    pane_id: &str,
) -> Result<(), ToolError> {
    const MAX_POLLS: usize = 72000;
    for _ in 0..MAX_POLLS {
        let capture = capture_container_tmux_pane(ctx, pane_id).await?;
        if latest_prompt_marker(&capture).is_some() {
            return Ok(());
        }
        sleep(Duration::from_millis(50)).await;
    }

    Err(ToolError::ExecutionFailed(
        "timed out waiting for tmux prompt initialization".into(),
    ))
}

fn parse_tmux_capture_output(
    capture: &str,
    start_command_id: u64,
    command: &str,
) -> Option<Result<ExecOutput, ToolError>> {
    let lines: Vec<&str> = capture.lines().collect();
    let start_idx = lines.iter().enumerate().rev().find_map(|(idx, line)| {
        parse_prompt_marker(line)
            .and_then(|marker| (marker.command_id == start_command_id).then_some(idx))
    });

    let Some(start_idx) = start_idx else {
        if let Some(latest) = latest_prompt_marker(capture) {
            if latest.command_id > start_command_id {
                return Some(Err(ToolError::ExecutionFailed(format!(
                    "tmux prompt marker {} is no longer visible in capture history",
                    start_command_id
                ))));
            }
        }
        return None;
    };

    let expected_completion_id = start_command_id.saturating_add(1);
    let completion_prompt = lines[start_idx + 1..]
        .iter()
        .enumerate()
        .filter_map(|(idx, line)| {
            parse_prompt_marker(line).map(|marker| (start_idx + 1 + idx, marker))
        })
        .find(|(_, marker)| marker.command_id > start_command_id);

    let Some((end_idx, completion_marker)) = completion_prompt else {
        return None;
    };
    if completion_marker.command_id != expected_completion_id {
        return Some(Err(ToolError::ExecutionFailed(format!(
            "unexpected tmux prompt command id: expected {}, got {}",
            expected_completion_id, completion_marker.command_id
        ))));
    }

    let mut output = lines[start_idx + 1..end_idx]
        .iter()
        .map(|line| (*line).to_string())
        .collect::<Vec<_>>();
    drop_echoed_command_line(&mut output, command);
    while output.first().is_some_and(|line| line.trim().is_empty()) {
        output.remove(0);
    }
    while output.last().is_some_and(|line| line.trim().is_empty()) {
        output.pop();
    }

    Some(Ok(ExecOutput {
        exit_code: completion_marker.exit_code,
        stdout: output.join("\n"),
        stderr: String::new(),
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PromptMarker {
    command_id: u64,
    exit_code: i32,
}

fn parse_prompt_marker(line: &str) -> Option<PromptMarker> {
    parse_prompt_marker_with_prefix(line, "[buddy ")
        // Compatibility with older tmux panes configured before rename.
        .or_else(|| parse_prompt_marker_with_prefix(line, "[agent "))
}

fn parse_prompt_marker_with_prefix(line: &str, prefix: &str) -> Option<PromptMarker> {
    let start = line.find(prefix)?;
    let tail = &line[start + prefix.len()..];
    let colon = tail.find(':')?;
    let command_id = tail[..colon].trim().parse::<u64>().ok()?;
    let after_colon = &tail[colon + 1..];
    let bracket = after_colon.find(']')?;
    let exit_code = after_colon[..bracket].trim().parse::<i32>().ok()?;
    Some(PromptMarker {
        command_id,
        exit_code,
    })
}

fn latest_prompt_marker(capture: &str) -> Option<PromptMarker> {
    capture.lines().rev().find_map(parse_prompt_marker)
}

fn drop_echoed_command_line(lines: &mut Vec<String>, command: &str) {
    let trimmed_command = command.trim();
    if trimmed_command.is_empty() {
        return;
    }
    if let Some(first) = lines.first() {
        if first.trim_end().ends_with(trimmed_command) {
            lines.remove(0);
        }
    }
}

fn unique_token(target: &str, remote_command: &str) -> String {
    let mut hasher = DefaultHasher::new();
    target.hash(&mut hasher);
    remote_command.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

async fn run_with_wait(
    fut: impl std::future::Future<Output = Result<ExecOutput, ToolError>>,
    wait: ShellWait,
    timeout_context: &str,
) -> Result<ExecOutput, ToolError> {
    match wait {
        ShellWait::Wait => fut.await,
        ShellWait::WaitWithTimeout(limit) => match timeout(limit, fut).await {
            Ok(out) => out,
            Err(_) => Err(ToolError::ExecutionFailed(format!(
                "{timeout_context} after {}",
                format_duration(limit)
            ))),
        },
        ShellWait::NoWait => Err(ToolError::ExecutionFailed(
            "run_shell wait=false requires a tmux-backed execution target".into(),
        )),
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    if secs == 0 {
        return format!("{millis}ms");
    }
    if millis == 0 {
        if secs % 3600 == 0 {
            return format!("{}h", secs / 3600);
        }
        if secs % 60 == 0 {
            return format!("{}m", secs / 60);
        }
        return format!("{secs}s");
    }
    format!("{secs}.{millis:03}s")
}

async fn run_process(
    program: &str,
    args: &[String],
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    let mut cmd = Command::new(program);
    // Background-task cancellation aborts in-flight futures; ensure child
    // processes are terminated when their owning future is dropped.
    cmd.kill_on_drop(true);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::ExecutionFailed(format!("{program}: {e}")))?;

    if let Some(input) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(input)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("{program}: {e}")))?;
        }
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("{program}: {e}")))?;

    Ok(ExecOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn ensure_success(output: ExecOutput, context: String) -> Result<ExecOutput, ToolError> {
    if output.exit_code == 0 {
        return Ok(output);
    }

    let mut details = if output.stderr.trim().is_empty() {
        output.stdout.trim().to_string()
    } else {
        output.stderr.trim().to_string()
    };
    if details.is_empty() {
        details = format!("command exited with {}", output.exit_code);
    }

    Err(ToolError::ExecutionFailed(format!("{context}: {details}")))
}

async fn detect_container_engine() -> Result<ContainerEngine, ToolError> {
    if let Some(version) = probe_version("docker").await? {
        let kind = docker_frontend_kind(&version);
        return Ok(ContainerEngine {
            command: "docker",
            kind,
        });
    }

    if probe_version("podman").await?.is_some() {
        return Ok(ContainerEngine {
            command: "podman",
            kind: ContainerEngineKind::Podman,
        });
    }

    Err(ToolError::ExecutionFailed(
        "neither `docker` nor `podman` was found in PATH".into(),
    ))
}

async fn probe_version(command: &str) -> Result<Option<String>, ToolError> {
    let output = match Command::new(command).arg("--version").output().await {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ToolError::ExecutionFailed(format!(
                "failed to probe {command}: {e}"
            )))
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(Some(format!("{stdout}\n{stderr}")))
}

fn docker_frontend_kind(version_output: &str) -> ContainerEngineKind {
    let text = version_output.to_ascii_lowercase();
    if text.contains("podman") {
        ContainerEngineKind::Podman
    } else {
        ContainerEngineKind::Docker
    }
}

fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        "''".into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_empty() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn quote_with_single_quote() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn detects_podman_from_docker_version_output() {
        let kind = docker_frontend_kind("Emulate Docker CLI using podman");
        assert_eq!(kind, ContainerEngineKind::Podman);
    }

    #[test]
    fn defaults_to_docker_when_podman_not_mentioned() {
        let kind = docker_frontend_kind("Docker version 26.1.0, build deadbeef");
        assert_eq!(kind, ContainerEngineKind::Docker);
    }

    #[test]
    fn tmux_session_name_is_stable_for_target() {
        let a = default_tmux_session_name("dev@host");
        let b = default_tmux_session_name("dev@host");
        assert_eq!(a, b);
        assert_eq!(a.len(), "buddy-0000".len());
    }

    #[test]
    fn default_local_tmux_session_name_matches_hash_strategy() {
        let name = default_local_tmux_session_name();
        assert!(name.starts_with("buddy-"));
        assert_eq!(name.len(), "buddy-0000".len());
        assert_eq!(name, default_tmux_session_name(LOCAL_TMUX_SESSION_SEED));
    }

    #[test]
    fn managed_tmux_window_name_detection_matches_buddy_shared() {
        assert!(is_managed_tmux_window_name("buddy-shared"));
        assert!(is_managed_tmux_window_name(" buddy-shared "));
        assert!(!is_managed_tmux_window_name("dev-shell"));
    }

    #[test]
    fn ensure_tmux_pane_script_uses_explicit_session_window_target() {
        let script = ensure_tmux_pane_script("buddy");
        assert!(script.contains("CREATED=0"));
        assert!(script.contains("CREATED=1"));
        assert!(script.contains("tmux new-session -d -s \"$SESSION\" -n \"$WINDOW\""));
        assert!(script.contains("tmux new-window -d -t \"$SESSION\" -n \"$WINDOW\""));
        assert!(!script.contains("tmux split-window -d -t \"$SESSION:$WINDOW\""));
    }

    #[test]
    fn parse_ensured_tmux_pane_reads_pane_and_created_flag() {
        assert_eq!(
            parse_ensured_tmux_pane("%3\n1"),
            Some(EnsuredTmuxPane {
                pane_id: "%3".to_string(),
                created: true,
            })
        );
        assert_eq!(
            parse_ensured_tmux_pane("%7\n0"),
            Some(EnsuredTmuxPane {
                pane_id: "%7".to_string(),
                created: false,
            })
        );
        assert!(parse_ensured_tmux_pane("").is_none());
        assert!(parse_ensured_tmux_pane("%3\n2").is_none());
    }

    #[test]
    fn local_tmux_summary_and_capture_availability() {
        let ctx = ExecutionContext {
            inner: Arc::new(LocalTmuxContext {
                tmux_session: "buddy-dev".to_string(),
                configured_tmux_pane: Mutex::new(None),
            }),
        };
        assert_eq!(ctx.summary(), "local (tmux:buddy-dev)");
        assert!(ctx.capture_pane_available());
        assert_eq!(
            ctx.tmux_attach_info(),
            Some(TmuxAttachInfo {
                session: "buddy-dev".to_string(),
                window: "buddy-shared",
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
                window: "buddy-shared",
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
            }),
        };
        assert_eq!(ctx.summary(), "ssh:dev@host (tmux:buddy-4a2f)");
        assert!(ctx.capture_pane_available());
        assert_eq!(
            ctx.tmux_attach_info(),
            Some(TmuxAttachInfo {
                session: "buddy-4a2f".to_string(),
                window: "buddy-shared",
                target: TmuxAttachTarget::Ssh {
                    target: "dev@host".to_string(),
                },
            })
        );
    }

    #[test]
    fn build_capture_pane_command_uses_defaults() {
        let cmd = build_capture_pane_command("%1", &CapturePaneOptions::default());
        assert_eq!(cmd, "tmux capture-pane -p -J -t '%1'");
    }

    #[test]
    fn local_tmux_is_disabled_by_default_in_unit_tests() {
        assert!(!local_tmux_allowed());
        assert!(local_tmux_pane_target().is_none());
    }

    #[test]
    fn full_history_capture_options_sets_explicit_history_bounds() {
        let opts = full_history_capture_options();
        assert_eq!(opts.start.as_deref(), Some("-"));
        assert_eq!(opts.end.as_deref(), Some("-"));
    }

    #[test]
    fn build_capture_pane_command_honors_requested_flags() {
        let cmd = build_capture_pane_command(
            "%11",
            &CapturePaneOptions {
                target: None,
                start: Some("-40".to_string()),
                end: Some("20".to_string()),
                join_wrapped_lines: false,
                preserve_trailing_spaces: true,
                include_escape_sequences: true,
                escape_non_printable: true,
                include_alternate_screen: true,
                delay: Duration::from_millis(250),
            },
        );
        assert_eq!(
            cmd,
            "tmux capture-pane -p -N -e -C -a -S '-40' -E '20' -t '%11'"
        );
    }

    #[test]
    fn build_tmux_send_key_commands_quote_targets_and_values() {
        assert_eq!(
            build_tmux_send_literal_command("%1", "abc"),
            "tmux send-keys -l -t '%1' 'abc'"
        );
        assert_eq!(
            build_tmux_send_keys_command("%1", &["C-c".to_string(), "Enter".to_string()]),
            "tmux send-keys -t '%1' 'C-c' 'Enter'"
        );
        assert_eq!(
            build_tmux_send_enter_command("%1"),
            "tmux send-keys -t '%1' Enter"
        );
    }

    #[test]
    fn fallback_from_alternate_screen_only_when_requested() {
        let err = ToolError::ExecutionFailed(
            "failed to capture tmux pane: no alternate screen".to_string(),
        );
        let mut with_alt = CapturePaneOptions::default();
        with_alt.include_alternate_screen = true;
        assert!(should_fallback_from_alternate_screen(&with_alt, &err));

        let without_alt = CapturePaneOptions::default();
        assert!(!should_fallback_from_alternate_screen(&without_alt, &err));
    }

    #[test]
    fn format_duration_prefers_human_units() {
        assert_eq!(format_duration(Duration::from_millis(250)), "250ms");
        assert_eq!(format_duration(Duration::from_secs(7)), "7s");
        assert_eq!(format_duration(Duration::from_secs(120)), "2m");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(format_duration(Duration::from_millis(1250)), "1.250s");
    }

    #[tokio::test]
    async fn run_with_wait_times_out_when_limit_hit() {
        let result = run_with_wait(
            async {
                sleep(Duration::from_millis(50)).await;
                Ok(ExecOutput {
                    exit_code: 0,
                    stdout: "ok".to_string(),
                    stderr: String::new(),
                })
            },
            ShellWait::WaitWithTimeout(Duration::from_millis(1)),
            "timed out",
        )
        .await;
        match result {
            Ok(_) => panic!("expected timeout error"),
            Err(err) => assert!(err.to_string().contains("timed out"), "got: {err}"),
        }
    }

    #[test]
    fn parse_prompt_marker_extracts_id_and_status() {
        let line = "[buddy 42: 127] dev@host:~$ ";
        let marker = parse_prompt_marker(line).expect("prompt marker");
        assert_eq!(marker.command_id, 42);
        assert_eq!(marker.exit_code, 127);
        let legacy_line = "[agent 9: 0] dev@host:~$ ";
        let legacy_marker = parse_prompt_marker(legacy_line).expect("legacy prompt marker");
        assert_eq!(legacy_marker.command_id, 9);
        assert_eq!(legacy_marker.exit_code, 0);
        assert!(parse_prompt_marker("dev@host:~$").is_none());
    }

    #[test]
    fn latest_prompt_marker_uses_most_recent_marker() {
        let capture = "[buddy 8: 0] one\noutput\n[buddy 9: 1] two";
        let marker = latest_prompt_marker(capture).expect("latest marker");
        assert_eq!(marker.command_id, 9);
        assert_eq!(marker.exit_code, 1);
    }

    #[test]
    fn parse_tmux_output_between_prompt_markers() {
        let capture = format!(
            "if [ \"${{BUDDY_PROMPT_LAYOUT:-}}\" != \"v3\" ]; then ... fi\n\
[buddy 1: 0] dev@host:~$ \n\
dev@host:~$ ls -la\n\
old-output\n\
[buddy 2: 0] dev@host:~$ \n\
dev@host:~$ pwd\n\
/home/mo\n\
[buddy 3: 0] dev@host:~$ \n\
dev@host:~$ ls -l\n\
total 8\n\
file.txt\n\
err.txt\n\
[buddy 4: 0] dev@host:~$ "
        );
        let out = parse_tmux_capture_output(&capture, 3, "ls -l")
            .expect("should parse prompts")
            .expect("should parse output");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("total 8"));
        assert!(out.stdout.contains("file.txt"));
        assert!(out.stdout.contains("err.txt"));
        assert!(!out.stdout.contains("BUDDY_PROMPT_LAYOUT"));
        assert!(!out.stdout.contains("old-output"));
        assert_eq!(out.stderr, "");
    }

    #[test]
    fn parse_tmux_output_waits_for_completion_prompt() {
        let capture = format!(
            "[buddy 10: 0] dev@host:~$ \n\
dev@host:~$ echo hi\n\
hi\n"
        );
        assert!(parse_tmux_capture_output(&capture, 10, "echo hi").is_none());
    }

    #[test]
    fn parse_tmux_output_reads_nonzero_exit_code_from_prompt() {
        let capture = format!(
            "[buddy 12: 0] dev@host:~$ \n\
dev@host:~$ missing_command\n\
zsh: command not found: missing_command\n\
[buddy 13: 127] dev@host:~$ "
        );
        let out = parse_tmux_capture_output(&capture, 12, "missing_command")
            .expect("should parse prompts")
            .expect("should parse output");
        assert_eq!(out.exit_code, 127);
        assert!(out.stdout.contains("command not found"));
    }

    #[test]
    fn parse_tmux_output_ignores_repeated_start_marker() {
        let capture = format!(
            "[buddy 30: 0] dev@host:~$ \n\
old output\n\
[buddy 30: 0] dev@host:~$ \n\
dev@host:~$ ls\n\
file.txt\n\
[buddy 31: 0] dev@host:~$ "
        );
        let out = parse_tmux_capture_output(&capture, 30, "ls")
            .expect("parse frame")
            .expect("parse output");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("file.txt"));
    }

    #[test]
    fn parse_tmux_output_rejects_unexpected_next_id() {
        let capture = format!(
            "[buddy 20: 0] dev@host:~$ \n\
dev@host:~$ echo hi\n\
hi\n\
[buddy 22: 0] dev@host:~$ "
        );
        let result = parse_tmux_capture_output(&capture, 20, "echo hi").expect("parse frame");
        match result {
            Ok(_) => panic!("should reject skipped command id"),
            Err(err) => assert!(err
                .to_string()
                .contains("unexpected tmux prompt command id")),
        }
    }

    #[test]
    fn parse_tmux_output_errors_if_start_marker_is_missing() {
        let capture = "[buddy 41: 0] dev@host:~$ \noutput\n[buddy 42: 0] dev@host:~$";
        let result = parse_tmux_capture_output(capture, 40, "ls").expect("parse frame");
        match result {
            Ok(_) => panic!("expected missing start marker error"),
            Err(err) => assert!(err
                .to_string()
                .contains("is no longer visible in capture history")),
        }
    }

    #[test]
    fn ssh_context_drop_triggers_control_cleanup() {
        use std::path::PathBuf;
        use std::sync::Arc;

        let observed = Arc::new(StdMutex::new(None::<(String, PathBuf)>));
        let observed_clone = Arc::clone(&observed);
        set_ssh_close_hook_for_tests(Some(Box::new(move |target, path| {
            *observed_clone.lock().expect("observed lock") =
                Some((target.to_string(), path.to_path_buf()));
        })));

        let control_path = PathBuf::from("/tmp/buddy-test-drop.sock");
        let ctx = SshContext {
            target: "dev@example.com".to_string(),
            control_path: control_path.clone(),
            tmux_session: None,
            configured_tmux_pane: Mutex::new(None),
        };
        drop(ctx);
        set_ssh_close_hook_for_tests(None);

        let captured = observed
            .lock()
            .expect("observed lock")
            .clone()
            .expect("drop should trigger cleanup");
        assert_eq!(captured.0, "dev@example.com");
        assert_eq!(captured.1, control_path);
    }
}
