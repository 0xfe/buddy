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
pub(crate) async fn run_ssh_tmux_process(
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
pub(crate) async fn run_local_tmux_process(
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
pub(crate) async fn run_container_tmux_process(
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
pub(crate) fn parse_tmux_capture_output(
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
pub(crate) struct PromptMarker {
    pub(crate) command_id: u64,
    pub(crate) exit_code: i32,
}

pub(crate) fn parse_prompt_marker(line: &str) -> Option<PromptMarker> {
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
pub(crate) fn latest_prompt_marker(capture: &str) -> Option<PromptMarker> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
