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
mod process;
mod tmux;
mod types;

use crate::error::ToolError;
use backend::local::ensure_not_in_managed_local_tmux_pane;
#[cfg(test)]
use backend::local::{is_managed_tmux_window_name, local_tmux_allowed, local_tmux_pane_target};
#[cfg(test)]
use backend::ssh::set_ssh_close_hook_for_tests;
use backend::ssh::{
    build_ssh_control_path, close_ssh_control_connection, default_tmux_session_name_for_agent,
};
use contracts::ExecutionBackendOps;
#[cfg(test)]
use process::docker_frontend_kind;
#[cfg(test)]
use process::format_duration;
#[cfg(test)]
use process::run_with_wait;
use process::{
    detect_container_engine, ensure_success, run_container_tmux_sh_process, run_process,
    run_sh_process, run_ssh_raw_process, shell_quote,
};
use std::sync::Arc;
#[cfg(test)]
use std::{path::PathBuf, sync::Mutex as StdMutex};
#[cfg(test)]
use tmux::capture::{
    build_capture_pane_command, full_history_capture_options, should_fallback_from_alternate_screen,
};
use tmux::pane::{ensure_container_tmux_pane, ensure_local_tmux_pane, ensure_tmux_pane};
#[cfg(test)]
use tmux::pane::{ensure_tmux_pane_script, parse_ensured_tmux_pane};
use tmux::prompt::{
    ensure_container_tmux_prompt_setup, ensure_local_tmux_prompt_setup, ensure_tmux_prompt_setup,
};
#[cfg(test)]
use tmux::run::{latest_prompt_marker, parse_prompt_marker, parse_tmux_capture_output};
#[cfg(test)]
use tmux::send_keys::{
    build_tmux_send_enter_command, build_tmux_send_keys_command, build_tmux_send_literal_command,
};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
#[cfg(test)]
use types::ContainerEngine;
#[cfg(test)]
use types::ContainerEngineKind;
#[cfg(test)]
use types::EnsuredTmuxPane;
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
    fn managed_tmux_window_name_detection_accepts_new_and_legacy_names() {
        assert!(is_managed_tmux_window_name("shared"));
        assert!(is_managed_tmux_window_name(" shared "));
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
        assert!(script.contains("tmux split-window -d -P -F '#{pane_id}' -t \"$SESSION:$WINDOW\""));
        assert!(script.contains("tmux select-pane -t \"$PANE\" -T \"$PANE_TITLE\""));
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
}
