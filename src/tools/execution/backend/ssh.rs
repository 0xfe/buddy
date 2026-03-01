//! SSH backend implementation and control-socket lifecycle helpers.

use crate::error::ToolError;
use async_trait::async_trait;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(test)]
use std::sync::{Mutex as StdMutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tmux::capture::run_remote_capture_pane;
use crate::tmux::management::{
    canonical_session_name, create_managed_pane_script, create_managed_session_script,
    kill_managed_pane_script, kill_managed_session_script, parse_created_pane,
    parse_created_session, parse_killed_pane, parse_resolved_target, resolve_managed_target_script,
};
use crate::tmux::pane::ensure_tmux_pane;
use crate::tmux::prompt::ensure_tmux_prompt_setup;
use crate::tmux::run::run_ssh_tmux_process;
use crate::tmux::send_keys::send_remote_tmux_keys;
use crate::tools::execution::contracts::{CommandBackend, ExecutionBackendOps};
use crate::tools::execution::file_io::{
    read_file_via_command_backend, write_file_via_command_backend,
};
use crate::tools::execution::process::{run_ssh_raw_process, run_with_wait};
use crate::tools::execution::types::{
    CapturePaneOptions, CreatedTmuxPane, CreatedTmuxSession, ExecOutput, ResolvedTmuxTarget,
    SendKeysOptions, ShellWait, SshContext, TmuxAttachInfo, TmuxAttachTarget, TmuxTargetSelector,
    TMUX_PANE_TITLE, TMUX_WINDOW_NAME,
};

impl SshContext {
    /// Run a command on the remote host, forcing tmux execution when a tmux
    /// session is configured for this connection.
    pub(in crate::tools::execution) async fn run_command(
        &self,
        remote_command: &str,
        stdin: Option<&[u8]>,
        wait: ShellWait,
    ) -> Result<ExecOutput, ToolError> {
        if let Some(session) = &self.tmux_session {
            // tmux-backed SSH uses managed pane execution for polling/no-wait behavior.
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
                // No-wait dispatch requires a persistent tmux pane.
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

    pub(in crate::tools::execution) async fn ensure_prompt_ready(
        &self,
        tmux_session: &str,
    ) -> Result<String, ToolError> {
        // Fast-path: configured pane still exists.
        let configured_pane = self.configured_tmux_pane.lock().await.clone();
        if let Some(pane_id) = configured_pane {
            if self.tmux_pane_exists(&pane_id).await? {
                return Ok(pane_id);
            }
        }

        // Slow-path: ensure pane and initialize prompt markers for new panes.
        let ensured = ensure_tmux_pane(
            &self.target,
            &self.control_path,
            tmux_session,
            &self.owner_prefix,
        )
        .await?;
        let mut configured = self.configured_tmux_pane.lock().await;
        if ensured.created {
            ensure_tmux_prompt_setup(&self.target, &self.control_path, &ensured.pane_id).await?;
        }
        if configured.as_deref() != Some(ensured.pane_id.as_str()) {
            *configured = Some(ensured.pane_id.clone());
        }
        Ok(ensured.pane_id)
    }

    async fn tmux_pane_exists(&self, pane_id: &str) -> Result<bool, ToolError> {
        // Probe remote pane IDs using existing SSH control socket.
        let pane_q = crate::tools::execution::process::shell_quote(pane_id);
        let probe =
            format!("tmux list-panes -a -F '#{{pane_id}}' | grep -Fx -- {pane_q} >/dev/null 2>&1");
        let output = run_ssh_raw_process(&self.target, &self.control_path, &probe, None).await?;
        Ok(output.exit_code == 0)
    }

    async fn resolve_target(
        &self,
        selector: TmuxTargetSelector,
        ensure_default_shared: bool,
    ) -> Result<ResolvedTmuxTarget, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "tmux target resolution requires an ssh tmux-backed execution context".into(),
            )
        })?;
        let wants_default_shared = selector.target.is_none()
            && selector
                .session
                .as_deref()
                .is_none_or(|session| session.trim() == tmux_session)
            && selector
                .pane
                .as_deref()
                .is_none_or(|pane| pane.trim() == TMUX_PANE_TITLE);
        if ensure_default_shared && wants_default_shared {
            let pane_id = self.ensure_prompt_ready(tmux_session).await?;
            return Ok(ResolvedTmuxTarget {
                session: tmux_session.to_string(),
                pane_id,
                pane_title: TMUX_PANE_TITLE.to_string(),
                is_default_shared: true,
            });
        }

        let script = resolve_managed_target_script(&self.owner_prefix, tmux_session, &selector)?;
        let output = run_ssh_raw_process(&self.target, &self.control_path, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to resolve managed tmux target".into(),
        )?;
        parse_resolved_target(&output.stdout)
            .ok_or_else(|| ToolError::ExecutionFailed("failed to parse managed tmux target".into()))
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

    fn startup_existing_tmux_pane(&self) -> Option<String> {
        self.startup_existing_tmux_pane.clone()
    }

    fn capture_pane_available(&self) -> bool {
        self.tmux_session.is_some()
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
        run_remote_capture_pane(
            &self.target,
            &self.control_path,
            &resolved.pane_id,
            &options,
        )
        .await
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
        send_remote_tmux_keys(
            &self.target,
            &self.control_path,
            &resolved.pane_id,
            &options,
        )
        .await?;
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
        run_ssh_tmux_process(
            &self.target,
            &self.control_path,
            &target.pane_id,
            command,
            None,
            wait,
        )
        .await
    }

    async fn read_file(&self, path: &str) -> Result<String, ToolError> {
        read_file_via_command_backend(self, path).await
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<(), ToolError> {
        write_file_via_command_backend(self, path, content).await
    }

    fn tmux_management_available(&self) -> bool {
        self.tmux_session.is_some()
    }

    async fn resolve_tmux_target(
        &self,
        selector: TmuxTargetSelector,
        ensure_default_shared: bool,
    ) -> Result<ResolvedTmuxTarget, ToolError> {
        self.resolve_target(selector, ensure_default_shared).await
    }

    async fn create_tmux_session(&self, session: String) -> Result<CreatedTmuxSession, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "tmux session management is unavailable: no tmux session for this ssh target"
                    .into(),
            )
        })?;
        let session = canonical_session_name(&self.owner_prefix, tmux_session, Some(&session))?;
        let script = create_managed_session_script(&self.owner_prefix, &session, self.max_sessions);
        let output = run_ssh_raw_process(&self.target, &self.control_path, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to create managed tmux session".into(),
        )?;
        let created = parse_created_session(&output.stdout).ok_or_else(|| {
            ToolError::ExecutionFailed("failed to parse created tmux session".into())
        })?;
        if created.created {
            ensure_tmux_prompt_setup(&self.target, &self.control_path, &created.pane_id).await?;
        }
        Ok(created)
    }

    async fn kill_tmux_session(&self, session: String) -> Result<String, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "tmux session management is unavailable: no tmux session for this ssh target"
                    .into(),
            )
        })?;
        let script = kill_managed_session_script(&self.owner_prefix, tmux_session, &session)?;
        let output = run_ssh_raw_process(&self.target, &self.control_path, &script, None).await?;
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
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "tmux pane management is unavailable: no tmux session for this ssh target".into(),
            )
        })?;
        let script = create_managed_pane_script(
            &self.owner_prefix,
            tmux_session,
            session.as_deref(),
            &pane,
            self.max_panes,
        )?;
        let output = run_ssh_raw_process(&self.target, &self.control_path, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to create managed tmux pane".into(),
        )?;
        let created = parse_created_pane(&output.stdout).ok_or_else(|| {
            ToolError::ExecutionFailed("failed to parse created tmux pane".into())
        })?;
        if created.created {
            ensure_tmux_prompt_setup(&self.target, &self.control_path, &created.pane_id).await?;
        }
        Ok(created)
    }

    async fn kill_tmux_pane(
        &self,
        session: Option<String>,
        pane: String,
    ) -> Result<String, ToolError> {
        let tmux_session = self.tmux_session.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "tmux pane management is unavailable: no tmux session for this ssh target".into(),
            )
        })?;
        let script =
            kill_managed_pane_script(&self.owner_prefix, tmux_session, session.as_deref(), &pane)?;
        let output = run_ssh_raw_process(&self.target, &self.control_path, &script, None).await?;
        let output = crate::tools::execution::process::ensure_success(
            output,
            "failed to kill managed tmux pane".into(),
        )?;
        let (session, pane_id) = parse_killed_pane(&output.stdout)
            .ok_or_else(|| ToolError::ExecutionFailed("failed to parse killed tmux pane".into()))?;
        Ok(format!("killed pane {pane_id} in session {session}"))
    }
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
pub(in crate::tools::execution) fn set_ssh_close_hook_for_tests(hook: Option<SshCloseHook>) {
    *ssh_close_hook_slot().lock().expect("ssh close hook lock") = hook;
}

pub(in crate::tools::execution) fn close_ssh_control_connection(target: &str, control_path: &Path) {
    #[cfg(test)]
    {
        // Test hook allows asserting cleanup without spawning ssh binaries.
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

pub(in crate::tools::execution) fn build_ssh_control_path(target: &str) -> PathBuf {
    // Include PID and timestamp to avoid collisions across concurrent runs.
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

pub(in crate::tools::execution) fn default_tmux_session_name_for_agent(agent_name: &str) -> String {
    // Prefix session names for easier discovery/cleanup.
    let suffix = sanitize_tmux_session_suffix(agent_name);
    format!("buddy-{suffix}")
}

fn sanitize_tmux_session_suffix(raw: &str) -> String {
    // Keep session suffix shell-safe and bounded for tmux compatibility.
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in raw.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
            continue;
        }

        if matches!(ch, '-' | '_') {
            if !previous_dash && !out.is_empty() {
                out.push(ch);
                previous_dash = true;
            }
            continue;
        }

        if !previous_dash && !out.is_empty() {
            out.push('-');
            previous_dash = true;
        }
    }

    let trimmed = out.trim_matches(['-', '_']).to_string();
    let normalized = if trimmed.is_empty() {
        "agent-mo".to_string()
    } else {
        trimmed
    };

    normalized.chars().take(48).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn tmux_session_name_uses_agent_name() {
        // Agent names should be normalized and prefixed consistently.
        assert_eq!(
            default_tmux_session_name_for_agent("agent-mo"),
            "buddy-agent-mo"
        );
        assert_eq!(
            default_tmux_session_name_for_agent("Ops Agent (Prod)"),
            "buddy-ops-agent-prod"
        );
    }

    #[test]
    fn tmux_session_name_falls_back_when_agent_name_is_empty() {
        // Empty agent names should map to deterministic fallback session suffix.
        assert_eq!(default_tmux_session_name_for_agent(""), "buddy-agent-mo");
        assert_eq!(default_tmux_session_name_for_agent("   "), "buddy-agent-mo");
    }

    #[test]
    fn ssh_context_drop_triggers_control_cleanup() {
        // Dropping SSH contexts should always invoke control-socket cleanup.
        let observed = Arc::new(std::sync::Mutex::new(None::<(String, PathBuf)>));
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
            owner_prefix: "buddy-agent-mo".to_string(),
            max_sessions: 1,
            max_panes: 5,
            configured_tmux_pane: Mutex::new(None),
            startup_existing_tmux_pane: None,
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

    #[tokio::test]
    async fn ssh_backend_rejects_no_wait_without_tmux() {
        // No-wait mode should be rejected when backend lacks tmux support.
        let ctx = SshContext {
            target: "dev@example.com".to_string(),
            control_path: PathBuf::from("/tmp/buddy-test.sock"),
            tmux_session: None,
            owner_prefix: "buddy-agent-mo".to_string(),
            max_sessions: 1,
            max_panes: 5,
            configured_tmux_pane: Mutex::new(None),
            startup_existing_tmux_pane: None,
        };

        match ctx.run_command("echo hi", None, ShellWait::NoWait).await {
            Ok(_) => panic!("no-wait should be rejected"),
            Err(err) => assert!(err
                .to_string()
                .contains("requires a tmux-backed execution target")),
        }
    }
}
