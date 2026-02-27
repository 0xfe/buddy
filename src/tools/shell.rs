//! Shell command execution tool.
//!
//! Runs a command via `sh -c` and returns stdout/stderr/exit code.
//! Optionally prompts the user for confirmation before execution.

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use super::execution::{ExecutionContext, ShellWait};
use super::{Tool, ToolContext, ToolStreamEvent};
use crate::error::ToolError;
use crate::render::Renderer;
use crate::textutil::truncate_with_suffix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters of command output to return.
const MAX_OUTPUT_LEN: usize = 4000;

/// Tool that runs shell commands and returns their output.
pub struct ShellTool {
    /// Whether to require user confirmation before execution.
    pub confirm: bool,
    /// Denylist patterns used to block dangerous commands.
    pub denylist: Vec<String>,
    /// Whether terminal UI should use color.
    pub color: bool,
    /// Where shell commands are actually executed (local/container/ssh).
    pub execution: ExecutionContext,
    /// Optional UI broker for foreground approval prompts in interactive mode.
    pub approval: Option<ShellApprovalBroker>,
}

#[derive(Deserialize)]
struct Args {
    command: String,
    wait: Option<WaitArg>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WaitArg {
    Bool(bool),
    Duration(String),
    Seconds(u64),
}

/// Foreground approval request emitted by `ShellTool` when confirmations are enabled.
#[derive(Debug)]
pub struct ShellApprovalRequest {
    command: String,
    response: oneshot::Sender<bool>,
}

impl ShellApprovalRequest {
    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn approve(self) {
        let _ = self.response.send(true);
    }

    pub fn deny(self) {
        let _ = self.response.send(false);
    }
}

/// Sender side for shell approval requests.
#[derive(Clone, Debug)]
pub struct ShellApprovalBroker {
    tx: mpsc::UnboundedSender<ShellApprovalRequest>,
}

impl ShellApprovalBroker {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ShellApprovalRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub async fn request(&self, command: String) -> Result<bool, ToolError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(ShellApprovalRequest {
                command,
                response: response_tx,
            })
            .map_err(|_| ToolError::ExecutionFailed("approval UI is unavailable".into()))?;
        response_rx.await.map_err(|_| {
            ToolError::ExecutionFailed("approval request was cancelled before resolution".into())
        })
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &'static str {
        "run_shell"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description:
                    "Run a shell command and return its output (stdout, stderr, exit code). The optional `wait` argument controls waiting behavior: `true` (default) waits until completion, `false` returns immediately (tmux-backed targets only) so you can poll with `capture-pane`, and a duration string like `10m` waits up to that timeout.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        },
                        "wait": {
                            "description": "Waiting mode: true (default) waits to completion; false dispatches and returns immediately (tmux targets only); or duration string like '30s', '10m', '1h' to wait with timeout.",
                            "oneOf": [
                                { "type": "boolean" },
                                { "type": "string" },
                                { "type": "integer", "minimum": 0 }
                            ]
                        }
                    },
                    "required": ["command"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, context: &ToolContext) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        if let Some(pattern) = matched_denylist_pattern(&args.command, &self.denylist) {
            return Err(ToolError::ExecutionFailed(format!(
                "command blocked by tools.shell_denylist pattern `{pattern}`"
            )));
        }
        let wait = parse_wait_mode(args.wait)?;
        context.emit(ToolStreamEvent::Started {
            detail: format!("run_shell: {}", args.command),
        });

        // Prompt for confirmation if enabled.
        if self.confirm {
            let approved = if let Some(approval) = &self.approval {
                approval.request(args.command.clone()).await?
            } else {
                eprint!("  Run: {} [y/N] ", args.command);
                let mut input = String::new();
                std::io::stdin()
                    .read_line(&mut input)
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
                input.trim().eq_ignore_ascii_case("y")
            };
            if !approved {
                context.emit(ToolStreamEvent::Completed {
                    detail: "run_shell denied by user".to_string(),
                });
                return Ok("Command execution denied by user.".to_string());
            }
        }

        // Shell commands can take a while; show a spinner while the command is running.
        // This spinner intentionally starts after any confirmation prompt.
        let renderer = Renderer::new(self.color);
        // In runtime/interactive mode a foreground liveness spinner already exists.
        // Avoid an extra background spinner thread that competes for stderr cursor control.
        let _progress =
            (!context.has_stream()).then(|| renderer.progress("running tool run_shell"));
        let output = self
            .execution
            .run_shell_command(&args.command, wait)
            .await?;
        if matches!(wait, ShellWait::NoWait) {
            if !output.stdout.trim().is_empty() {
                context.emit(ToolStreamEvent::Info {
                    message: output.stdout.clone(),
                });
            }
            context.emit(ToolStreamEvent::Completed {
                detail: format!("run_shell completed with exit code {}", output.exit_code),
            });
            return Ok(output.stdout);
        }
        let stdout_text = truncate_output(&output.stdout, MAX_OUTPUT_LEN);
        let stderr_text = truncate_output(&output.stderr, MAX_OUTPUT_LEN);
        if !stdout_text.trim().is_empty() {
            context.emit(ToolStreamEvent::StdoutChunk {
                chunk: stdout_text.clone(),
            });
        }
        if !stderr_text.trim().is_empty() {
            context.emit(ToolStreamEvent::StderrChunk {
                chunk: stderr_text.clone(),
            });
        }
        context.emit(ToolStreamEvent::Completed {
            detail: format!("run_shell completed with exit code {}", output.exit_code),
        });

        Ok(format!(
            "exit code: {}\nstdout:\n{stdout_text}\nstderr:\n{stderr_text}",
            output.exit_code
        ))
    }
}

fn parse_wait_mode(wait: Option<WaitArg>) -> Result<ShellWait, ToolError> {
    match wait {
        None | Some(WaitArg::Bool(true)) => Ok(ShellWait::Wait),
        Some(WaitArg::Bool(false)) => Ok(ShellWait::NoWait),
        Some(WaitArg::Seconds(secs)) => Ok(ShellWait::WaitWithTimeout(Duration::from_secs(secs))),
        Some(WaitArg::Duration(raw)) => parse_duration_arg(&raw)
            .map(ShellWait::WaitWithTimeout)
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "invalid wait duration; use forms like 30s, 10m, 1h, 500ms".into(),
                )
            }),
    }
}

fn parse_duration_arg(input: &str) -> Option<Duration> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }

    let (digits, unit) = if s.ends_with("ms") {
        (&s[..s.len() - 2], "ms")
    } else if let Some(last) = s.chars().last() {
        if last.is_ascii_alphabetic() {
            (&s[..s.len() - 1], &s[s.len() - 1..])
        } else {
            (s.as_str(), "s")
        }
    } else {
        return None;
    };
    let value = digits.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(value)),
        "s" => Some(Duration::from_secs(value)),
        "m" => value.checked_mul(60).map(Duration::from_secs),
        "h" => value
            .checked_mul(60)
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        "d" => value
            .checked_mul(24)
            .and_then(|v| v.checked_mul(60))
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        _ => None,
    }
}

fn truncate_output(s: &str, max: usize) -> String {
    truncate_with_suffix_by_bytes(s, max, "...[truncated]")
}

fn matched_denylist_pattern(command: &str, denylist: &[String]) -> Option<String> {
    let lowered = command.to_ascii_lowercase();
    denylist
        .iter()
        .map(|pattern| pattern.trim())
        .filter(|pattern| !pattern.is_empty())
        .find(|pattern| lowered.contains(&pattern.to_ascii_lowercase()))
        .map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_exactly_at_limit_unchanged() {
        assert_eq!(truncate_output("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string_adds_marker() {
        let result = truncate_output("xxxxxxxxxx", 5);
        assert_eq!(result, "xxxxx...[truncated]");
    }

    #[test]
    fn truncate_handles_utf8_without_panicking() {
        let result = truncate_output("🙂🙂🙂", 5);
        assert_eq!(result, "🙂...[truncated]");
    }

    #[test]
    fn parse_wait_mode_defaults_to_wait() {
        let mode = parse_wait_mode(None).expect("mode");
        assert!(matches!(mode, ShellWait::Wait));
    }

    #[test]
    fn parse_wait_mode_accepts_false_for_nowait() {
        let mode = parse_wait_mode(Some(WaitArg::Bool(false))).expect("mode");
        assert!(matches!(mode, ShellWait::NoWait));
    }

    #[test]
    fn parse_wait_mode_accepts_duration_strings() {
        let mode = parse_wait_mode(Some(WaitArg::Duration("10m".into()))).expect("mode");
        assert!(matches!(mode, ShellWait::WaitWithTimeout(d) if d == Duration::from_secs(600)));
    }

    #[test]
    fn parse_wait_mode_rejects_invalid_duration() {
        let err = parse_wait_mode(Some(WaitArg::Duration("bad".into()))).expect_err("error");
        assert!(err.to_string().contains("invalid wait duration"));
    }

    #[test]
    fn name_is_run_shell() {
        assert_eq!(
            ShellTool {
                confirm: false,
                denylist: Vec::new(),
                color: false,
                execution: ExecutionContext::local(),
                approval: None,
            }
            .name(),
            "run_shell"
        );
    }

    #[tokio::test]
    async fn execute_invalid_json_returns_error() {
        let err = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute("not json", &ToolContext::empty())
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid arguments"));
    }

    #[tokio::test]
    async fn execute_echo_command() {
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command": "echo hello"}"#, &ToolContext::empty())
        .await
        .unwrap();
        assert!(result.contains("exit code: 0"), "got: {result}");
        assert!(result.contains("hello"), "got: {result}");
    }

    #[tokio::test]
    async fn execute_failing_command_reports_exit_code() {
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command": "exit 42"}"#, &ToolContext::empty())
        .await
        .unwrap();
        assert!(result.contains("exit code: 42"), "got: {result}");
    }

    #[tokio::test]
    async fn execute_stderr_captured() {
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command": "echo err >&2"}"#, &ToolContext::empty())
        .await
        .unwrap();
        assert!(result.contains("err"), "got: {result}");
    }

    #[tokio::test]
    async fn execute_wait_false_requires_tmux_or_dispatches() {
        let real_tmux_enabled = std::env::var("BUDDY_TEST_USE_REAL_TMUX")
            .or_else(|_| std::env::var("AGENT_TEST_USE_REAL_TMUX"))
            .ok()
            .is_some_and(|v| v.trim() == "1");
        let outcome = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(
            r#"{"command":"echo hi","wait":false}"#,
            &ToolContext::empty(),
        )
        .await;
        if real_tmux_enabled
            && std::env::var("TMUX_PANE")
                .ok()
                .is_some_and(|v| !v.trim().is_empty())
        {
            let out = outcome.expect("wait=false should dispatch inside tmux");
            assert!(
                out.contains("command dispatched"),
                "unexpected output: {out}"
            );
        } else {
            let err = outcome.expect_err("wait=false should fail without tmux");
            assert!(err.to_string().contains("tmux"), "unexpected error: {err}");
        }
    }

    #[tokio::test]
    async fn execute_wait_duration_can_timeout() {
        let err = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(
            r#"{"command":"sleep 1","wait":"1ms"}"#,
            &ToolContext::empty(),
        )
        .await
        .expect_err("timeout expected");
        assert!(
            err.to_string().contains("timed out"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn execute_confirm_approved_via_broker_runs_command() {
        let (broker, mut rx) = ShellApprovalBroker::channel();

        let join = tokio::spawn(async move {
            ShellTool {
                confirm: true,
                denylist: Vec::new(),
                color: false,
                execution: ExecutionContext::local(),
                approval: Some(broker),
            }
            .execute(r#"{"command":"echo approved"}"#, &ToolContext::empty())
            .await
        });

        let req = rx.recv().await.expect("approval request expected");
        assert_eq!(req.command(), "echo approved");
        req.approve();

        let result = join.await.expect("join should succeed").unwrap();
        assert!(result.contains("exit code: 0"), "got: {result}");
        assert!(result.contains("approved"), "got: {result}");
    }

    #[tokio::test]
    async fn execute_confirm_denied_via_broker_skips_command() {
        let (broker, mut rx) = ShellApprovalBroker::channel();

        let join = tokio::spawn(async move {
            ShellTool {
                confirm: true,
                denylist: Vec::new(),
                color: false,
                execution: ExecutionContext::local(),
                approval: Some(broker),
            }
            .execute(r#"{"command":"echo denied"}"#, &ToolContext::empty())
            .await
        });

        let req = rx.recv().await.expect("approval request expected");
        assert_eq!(req.command(), "echo denied");
        req.deny();

        let result = join.await.expect("join should succeed").unwrap();
        assert_eq!(result, "Command execution denied by user.");
    }

    #[tokio::test]
    async fn execute_blocks_commands_matching_denylist() {
        let err = ShellTool {
            confirm: false,
            denylist: vec!["rm -rf /".to_string()],
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command":"rm -rf /tmp/test"}"#, &ToolContext::empty())
        .await
        .expect_err("denylist should block this command");
        assert!(
            err.to_string().contains("tools.shell_denylist"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn execute_emits_stream_events() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let context = ToolContext::with_stream(tx);
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command":"echo streamed"}"#, &context)
        .await
        .expect("shell command should succeed");
        assert!(result.contains("exit code: 0"), "got: {result}");

        let mut saw_started = false;
        let mut saw_stdout = false;
        let mut saw_completed = false;
        while let Ok(event) = rx.try_recv() {
            match event {
                ToolStreamEvent::Started { .. } => saw_started = true,
                ToolStreamEvent::StdoutChunk { chunk } if chunk.contains("streamed") => {
                    saw_stdout = true;
                }
                ToolStreamEvent::Completed { .. } => saw_completed = true,
                _ => {}
            }
        }
        assert!(saw_started, "missing started stream event");
        assert!(saw_stdout, "missing stdout stream event");
        assert!(saw_completed, "missing completed stream event");
    }

    #[cfg(feature = "fuzz-tests")]
    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn parse_duration_arg_accepts_supported_units(
                value in 1u64..10_000u64,
                unit in prop_oneof![Just("ms"), Just("s"), Just("m"), Just("h"), Just("d")]
            ) {
                let raw = format!("{value}{unit}");
                let parsed = parse_duration_arg(&raw).expect("duration should parse");
                let expected = match unit {
                    "ms" => Duration::from_millis(value),
                    "s" => Duration::from_secs(value),
                    "m" => Duration::from_secs(value.saturating_mul(60)),
                    "h" => Duration::from_secs(value.saturating_mul(3600)),
                    "d" => Duration::from_secs(value.saturating_mul(86_400)),
                    _ => unreachable!("covered by generator"),
                };
                prop_assert_eq!(parsed, expected);
            }

            #[test]
            fn parse_duration_arg_rejects_unknown_suffixes(
                value in 1u64..10_000u64,
                suffix in proptest::string::string_regex("[a-z]{1,3}").expect("regex")
            ) {
                prop_assume!(suffix != "ms" && suffix != "s" && suffix != "m" && suffix != "h" && suffix != "d");
                let raw = format!("{value}{suffix}");
                prop_assert_eq!(parse_duration_arg(&raw), None);
            }
        }
    }
}
