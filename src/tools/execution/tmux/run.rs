//! Tmux command execution loops and pane-output parsing.

use crate::error::ToolError;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::time::{sleep, Duration, Instant};

use crate::tools::execution::process::{
    ensure_success, format_duration, run_container_tmux_sh_process, run_sh_process,
    run_ssh_raw_process, shell_quote,
};
use crate::tools::execution::types::{ContainerTmuxContext, ExecOutput, ShellWait};

use super::capture::{capture_container_tmux_pane, capture_local_tmux_pane, capture_tmux_pane};
use super::send_keys::{send_container_tmux_line, send_local_tmux_line, send_tmux_line};

/// Execute a command in ssh tmux pane and parse result from prompt markers.
pub(in crate::tools::execution) async fn run_ssh_tmux_process(
    target: &str,
    control_path: &std::path::Path,
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

/// Execute a command in local tmux pane and parse result from prompt markers.
pub(in crate::tools::execution) async fn run_local_tmux_process(
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

/// Execute a command in container tmux pane and parse result from prompt markers.
pub(in crate::tools::execution) async fn run_container_tmux_process(
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

async fn wait_for_tmux_result(
    target: &str,
    control_path: &std::path::Path,
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

/// Parse tmux pane capture between start and next prompt markers.
pub(in crate::tools::execution) fn parse_tmux_capture_output(
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
pub(in crate::tools::execution) struct PromptMarker {
    pub(in crate::tools::execution) command_id: u64,
    pub(in crate::tools::execution) exit_code: i32,
}

pub(in crate::tools::execution) fn parse_prompt_marker(line: &str) -> Option<PromptMarker> {
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

/// Return the most recent prompt marker visible in capture output.
pub(in crate::tools::execution) fn latest_prompt_marker(capture: &str) -> Option<PromptMarker> {
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
