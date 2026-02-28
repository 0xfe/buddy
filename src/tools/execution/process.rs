//! Process and shell execution helpers shared by execution backends.

use crate::error::ToolError;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

use super::types::{
    ContainerContext, ContainerEngine, ContainerEngineKind, ContainerTmuxContext, ExecOutput,
    ShellWait,
};

/// Run a container shell command for non-tmux backends.
pub(super) async fn run_container_sh_process(
    ctx: &ContainerContext,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_container_sh_process_with(&ctx.engine, &ctx.container, command, stdin).await
}

/// Run a container shell command for tmux-backed container backends.
pub(super) async fn run_container_tmux_sh_process(
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

/// Run a local shell command.
pub(super) async fn run_sh_process(
    shell: &str,
    command: &str,
    stdin: Option<&[u8]>,
) -> Result<ExecOutput, ToolError> {
    run_process(shell, &["-c".into(), command.into()], stdin).await
}

/// Run a raw ssh command using the shared control socket.
pub(super) async fn run_ssh_raw_process(
    target: &str,
    control_path: &std::path::Path,
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

/// Wrap command futures with caller-selected wait semantics.
pub(super) async fn run_with_wait(
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

/// Human-oriented timeout formatting used in error messages.
pub(super) fn format_duration(duration: Duration) -> String {
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

/// Spawn and wait for a process, optionally piping stdin.
pub(super) async fn run_process(
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

/// Convert non-zero command status into contextual execution errors.
pub(super) fn ensure_success(output: ExecOutput, context: String) -> Result<ExecOutput, ToolError> {
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

/// Detect docker/podman frontend availability and compatibility mode.
pub(super) async fn detect_container_engine() -> Result<ContainerEngine, ToolError> {
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

/// Infer container frontend compatibility when docker binary is present.
pub(super) fn docker_frontend_kind(version_output: &str) -> ContainerEngineKind {
    let text = version_output.to_ascii_lowercase();
    if text.contains("podman") {
        ContainerEngineKind::Podman
    } else {
        ContainerEngineKind::Docker
    }
}

/// Shell-safe single-quote escaping.
pub(super) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        "''".into()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

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
}
