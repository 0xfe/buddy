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

use crate::tools::execution::contracts::{CommandBackend, ExecutionBackendOps};
use crate::tools::execution::file_io::{
    read_file_via_command_backend, write_file_via_command_backend,
};
use crate::tools::execution::process::{run_ssh_raw_process, run_with_wait};
use crate::tools::execution::tmux::capture::run_remote_capture_pane;
use crate::tools::execution::tmux::pane::ensure_tmux_pane;
use crate::tools::execution::tmux::prompt::ensure_tmux_prompt_setup;
use crate::tools::execution::tmux::run::run_ssh_tmux_process;
use crate::tools::execution::tmux::send_keys::send_remote_tmux_keys;
use crate::tools::execution::types::{
    CapturePaneOptions, ExecOutput, SendKeysOptions, ShellWait, SshContext, TmuxAttachInfo,
    TmuxAttachTarget, TMUX_WINDOW_NAME,
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

    pub(in crate::tools::execution) async fn ensure_prompt_ready(
        &self,
        tmux_session: &str,
    ) -> Result<String, ToolError> {
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
    let suffix = sanitize_tmux_session_suffix(agent_name);
    format!("buddy-{suffix}")
}

fn sanitize_tmux_session_suffix(raw: &str) -> String {
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
        assert_eq!(default_tmux_session_name_for_agent(""), "buddy-agent-mo");
        assert_eq!(default_tmux_session_name_for_agent("   "), "buddy-agent-mo");
    }

    #[test]
    fn ssh_context_drop_triggers_control_cleanup() {
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
        let ctx = SshContext {
            target: "dev@example.com".to_string(),
            control_path: PathBuf::from("/tmp/buddy-test.sock"),
            tmux_session: None,
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
