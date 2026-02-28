//! Shell command execution tool.
//!
//! Runs a command via `sh -c` and returns stdout/stderr/exit code.
//! Optionally prompts the user for confirmation before execution.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use super::execution::{ExecutionContext, ShellWait};
use super::result_envelope::wrap_result;
use super::{Tool, ToolContext, ToolStreamEvent};
use crate::error::ToolError;
use crate::textutil::truncate_with_suffix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};
use crate::ui::render::Renderer;

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
    /// Shell snippet to execute via configured backend.
    command: String,
    /// Optional wait behavior override.
    wait: Option<WaitArg>,
    /// Declared risk classification.
    risk: RiskLevel,
    /// Whether command mutates state.
    mutation: bool,
    /// Whether command uses privilege escalation.
    privesc: bool,
    /// Human rationale recorded alongside approval metadata.
    why: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WaitArg {
    /// `true` waits, `false` dispatches without waiting (tmux only).
    Bool(bool),
    /// Duration strings like `30s`, `10m`, `500ms`.
    Duration(String),
    /// Raw seconds timeout.
    Seconds(u64),
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// Stable lowercase string form used by logs and prompts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShellApprovalMetadata {
    /// Declared risk level from tool arguments.
    risk: RiskLevel,
    /// Mutation flag from tool arguments.
    mutation: bool,
    /// Privilege-escalation flag from tool arguments.
    privesc: bool,
    /// Human-readable reason from tool arguments.
    why: String,
}

impl ShellApprovalMetadata {
    /// Risk level requested by caller.
    pub fn risk(&self) -> RiskLevel {
        self.risk
    }

    /// Whether command was declared as mutating.
    pub fn mutation(&self) -> bool {
        self.mutation
    }

    /// Whether command was declared as privilege escalating.
    pub fn privesc(&self) -> bool {
        self.privesc
    }

    /// Human rationale accompanying the approval request.
    pub fn why(&self) -> &str {
        &self.why
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ShellToolResultPayload {
    /// Process exit code.
    exit_code: i32,
    /// Captured stdout, truncated to safe size.
    stdout: String,
    /// Captured stderr, truncated to safe size.
    stderr: String,
}

/// Foreground approval request emitted by `ShellTool` when confirmations are enabled.
#[derive(Debug)]
pub struct ShellApprovalRequest {
    /// Shell command awaiting operator decision.
    command: String,
    /// Optional metadata shown in interactive approval UI.
    metadata: Option<ShellApprovalMetadata>,
    /// One-shot responder for approve/deny decision.
    response: oneshot::Sender<bool>,
}

impl ShellApprovalRequest {
    /// Construct a new approval request.
    fn new(
        command: String,
        metadata: Option<ShellApprovalMetadata>,
        response: oneshot::Sender<bool>,
    ) -> Self {
        Self {
            command,
            metadata,
            response,
        }
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    /// Optional metadata attached to this request.
    pub fn metadata(&self) -> Option<&ShellApprovalMetadata> {
        self.metadata.as_ref()
    }

    /// Approve command execution.
    pub fn approve(self) {
        let _ = self.response.send(true);
    }

    /// Deny command execution.
    pub fn deny(self) {
        let _ = self.response.send(false);
    }
}

/// Sender side for shell approval requests.
#[derive(Clone, Debug)]
pub struct ShellApprovalBroker {
    /// Channel used by tools to publish foreground approval requests.
    tx: mpsc::UnboundedSender<ShellApprovalRequest>,
}

impl ShellApprovalBroker {
    /// Create a broker and paired receiver consumed by the UI/event loop.
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<ShellApprovalRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Send an approval request and await operator decision.
    pub async fn request(
        &self,
        command: String,
        metadata: Option<ShellApprovalMetadata>,
    ) -> Result<bool, ToolError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.tx
            .send(ShellApprovalRequest::new(command, metadata, response_tx))
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
                        "risk": {
                            "type": "string",
                            "enum": ["low", "medium", "high"],
                            "description": "Estimated risk level for this command."
                        },
                        "mutation": {
                            "type": "boolean",
                            "description": "True when the command mutates system state (writes files, changes configuration, etc). Writing only under /tmp can be treated as non-mutation."
                        },
                        "privesc": {
                            "type": "boolean",
                            "description": "True when command uses privilege escalation (for example sudo/su)."
                        },
                        "why": {
                            "type": "string",
                            "description": "Short reason for running the command, including risk justification and what is being mutated when mutation=true."
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
                    "required": ["command", "risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, context: &ToolContext) -> Result<String, ToolError> {
        // Parse structured arguments and validate required rationale.
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        if args.why.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "run_shell.why must be a non-empty string".to_string(),
            ));
        }
        // Denylist is checked before any execution side effects.
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
            // Bubble argument metadata into confirmation surfaces.
            let metadata = ShellApprovalMetadata {
                risk: args.risk,
                mutation: args.mutation,
                privesc: args.privesc,
                why: args.why.clone(),
            };
            let approved = if let Some(approval) = &self.approval {
                approval
                    .request(args.command.clone(), Some(metadata))
                    .await?
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
                return wrap_result("Command execution denied by user.");
            }
        }

        // Shell commands can take a while; show a spinner while the command is running.
        // This spinner intentionally starts after any confirmation prompt.
        let renderer = Renderer::new(self.color);
        // In runtime/interactive mode a foreground liveness spinner already exists.
        // Avoid an extra background spinner thread that competes for stderr cursor control.
        let _progress =
            (!context.has_stream()).then(|| renderer.progress("running tool run_shell"));
        // Execute using configured backend and wait semantics.
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
            return wrap_result(output.stdout);
        }
        // Truncate textual streams before emitting/serializing.
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

        wrap_result(ShellToolResultPayload {
            exit_code: output.exit_code,
            stdout: stdout_text,
            stderr: stderr_text,
        })
    }
}

fn parse_wait_mode(wait: Option<WaitArg>) -> Result<ShellWait, ToolError> {
    // Keep bool semantics backward-compatible while supporting richer timeouts.
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
    // Small hand-rolled parser avoids extra dependencies in this hot path.
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
    // Output limits protect prompt/context window budget.
    truncate_with_suffix_by_bytes(s, max, "...[truncated]")
}

fn matched_denylist_pattern(command: &str, denylist: &[String]) -> Option<String> {
    // Case-insensitive substring match keeps configuration simple.
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
    use serde_json::json;

    fn shell_args(command: &str) -> String {
        // Build minimal valid run_shell payload used by most tests.
        json!({
            "command": command,
            "risk": "low",
            "mutation": false,
            "privesc": false,
            "why": "run a safe read-only command"
        })
        .to_string()
    }

    fn shell_args_with_wait(command: &str, wait: serde_json::Value) -> String {
        // Build valid run_shell payload with explicit wait override.
        json!({
            "command": command,
            "wait": wait,
            "risk": "low",
            "mutation": false,
            "privesc": false,
            "why": "run a safe read-only command"
        })
        .to_string()
    }

    fn parse_result_envelope(result: &str) -> serde_json::Value {
        // Decode tool result envelope produced by wrap_result.
        serde_json::from_str(result).expect("result envelope json")
    }

    #[test]
    fn truncate_short_string_unchanged() {
        // Short output should pass through unchanged.
        assert_eq!(truncate_output("hello", 100), "hello");
    }

    #[test]
    fn truncate_exactly_at_limit_unchanged() {
        // Output at exact limit should not be modified.
        assert_eq!(truncate_output("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_string_adds_marker() {
        // Over-limit output should include truncation marker.
        let result = truncate_output("xxxxxxxxxx", 5);
        assert_eq!(result, "xxxxx...[truncated]");
    }

    #[test]
    fn truncate_handles_utf8_without_panicking() {
        // Truncation must preserve UTF-8 validity.
        let result = truncate_output("🙂🙂🙂", 5);
        assert_eq!(result, "🙂...[truncated]");
    }

    #[test]
    fn parse_wait_mode_defaults_to_wait() {
        // Missing wait argument should default to blocking mode.
        let mode = parse_wait_mode(None).expect("mode");
        assert!(matches!(mode, ShellWait::Wait));
    }

    #[test]
    fn parse_wait_mode_accepts_false_for_nowait() {
        // Boolean false should map to no-wait dispatch mode.
        let mode = parse_wait_mode(Some(WaitArg::Bool(false))).expect("mode");
        assert!(matches!(mode, ShellWait::NoWait));
    }

    #[test]
    fn parse_wait_mode_accepts_duration_strings() {
        // Duration strings should map to timeout wait mode.
        let mode = parse_wait_mode(Some(WaitArg::Duration("10m".into()))).expect("mode");
        assert!(matches!(mode, ShellWait::WaitWithTimeout(d) if d == Duration::from_secs(600)));
    }

    #[test]
    fn parse_wait_mode_rejects_invalid_duration() {
        // Invalid duration strings should return a validation error.
        let err = parse_wait_mode(Some(WaitArg::Duration("bad".into()))).expect_err("error");
        assert!(err.to_string().contains("invalid wait duration"));
    }

    #[test]
    fn name_is_run_shell() {
        // Tool name must match the registered function name.
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
        // Malformed JSON arguments should return invalid-arguments errors.
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
    async fn execute_missing_required_metadata_returns_error() {
        // Metadata fields are required for policy/audit reasons.
        let err = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(r#"{"command":"echo hi"}"#, &ToolContext::empty())
        .await
        .expect_err("missing metadata should fail");
        assert!(err.to_string().contains("invalid arguments"));
    }

    #[tokio::test]
    async fn execute_echo_command() {
        // Successful commands should report stdout and zero exit code.
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(&shell_args("echo hello"), &ToolContext::empty())
        .await
        .unwrap();
        let value = parse_result_envelope(&result);
        assert_eq!(value["result"]["exit_code"], 0);
        assert!(value["result"]["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("hello")));
    }

    #[tokio::test]
    async fn execute_failing_command_reports_exit_code() {
        // Non-zero exits should be preserved in structured output.
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(&shell_args("exit 42"), &ToolContext::empty())
        .await
        .unwrap();
        let value = parse_result_envelope(&result);
        assert_eq!(value["result"]["exit_code"], 42);
    }

    #[tokio::test]
    async fn execute_stderr_captured() {
        // Stderr output should be captured separately from stdout.
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(&shell_args("echo err >&2"), &ToolContext::empty())
        .await
        .unwrap();
        let value = parse_result_envelope(&result);
        assert!(value["result"]["stderr"]
            .as_str()
            .is_some_and(|text| text.contains("err")));
    }

    #[tokio::test]
    async fn execute_wait_false_requires_tmux_or_dispatches() {
        // wait=false should dispatch only on tmux-capable backends.
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
            &shell_args_with_wait("echo hi", json!(false)),
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
                parse_result_envelope(&out)["result"]
                    .as_str()
                    .is_some_and(|text| text.contains("command dispatched")),
                "unexpected output: {out}"
            );
        } else {
            let err = outcome.expect_err("wait=false should fail without tmux");
            assert!(err.to_string().contains("tmux"), "unexpected error: {err}");
        }
    }

    #[tokio::test]
    async fn execute_wait_duration_can_timeout() {
        // Timeout waits should fail when command exceeds requested limit.
        let err = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(
            &shell_args_with_wait("sleep 1", json!("1ms")),
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
        // Approved broker requests should execute and return normal results.
        let (broker, mut rx) = ShellApprovalBroker::channel();

        let join = tokio::spawn(async move {
            ShellTool {
                confirm: true,
                denylist: Vec::new(),
                color: false,
                execution: ExecutionContext::local(),
                approval: Some(broker),
            }
            .execute(&shell_args("echo approved"), &ToolContext::empty())
            .await
        });

        let req = rx.recv().await.expect("approval request expected");
        assert_eq!(req.command(), "echo approved");
        let metadata = req.metadata().expect("metadata expected");
        assert_eq!(metadata.risk(), RiskLevel::Low);
        assert!(!metadata.mutation());
        assert!(!metadata.privesc());
        req.approve();

        let result = join.await.expect("join should succeed").unwrap();
        let value = parse_result_envelope(&result);
        assert_eq!(value["result"]["exit_code"], 0);
        assert!(value["result"]["stdout"]
            .as_str()
            .is_some_and(|text| text.contains("approved")));
    }

    #[tokio::test]
    async fn execute_confirm_denied_via_broker_skips_command() {
        // Denied broker requests should skip execution and return denial message.
        let (broker, mut rx) = ShellApprovalBroker::channel();

        let join = tokio::spawn(async move {
            ShellTool {
                confirm: true,
                denylist: Vec::new(),
                color: false,
                execution: ExecutionContext::local(),
                approval: Some(broker),
            }
            .execute(&shell_args("echo denied"), &ToolContext::empty())
            .await
        });

        let req = rx.recv().await.expect("approval request expected");
        assert_eq!(req.command(), "echo denied");
        req.deny();

        let result = join.await.expect("join should succeed").unwrap();
        let value = parse_result_envelope(&result);
        assert_eq!(value["result"], "Command execution denied by user.");
    }

    #[tokio::test]
    async fn execute_blocks_commands_matching_denylist() {
        // Denylist matches should block execution before running the command.
        let err = ShellTool {
            confirm: false,
            denylist: vec!["rm -rf /".to_string()],
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(&shell_args("rm -rf /tmp/test"), &ToolContext::empty())
        .await
        .expect_err("denylist should block this command");
        assert!(
            err.to_string().contains("tools.shell_denylist"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn execute_emits_stream_events() {
        // Streaming mode should emit lifecycle and output events.
        let (tx, mut rx) = mpsc::unbounded_channel();
        let context = ToolContext::with_stream(tx);
        let result = ShellTool {
            confirm: false,
            denylist: Vec::new(),
            color: false,
            execution: ExecutionContext::local(),
            approval: None,
        }
        .execute(&shell_args("echo streamed"), &context)
        .await
        .expect("shell command should succeed");
        let value = parse_result_envelope(&result);
        assert_eq!(value["result"]["exit_code"], 0);

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
                // Any supported suffix should parse to the expected duration.
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
                // Unsupported suffixes should never parse.
                prop_assume!(suffix != "ms" && suffix != "s" && suffix != "m" && suffix != "h" && suffix != "d");
                let raw = format!("{value}{suffix}");
                prop_assert_eq!(parse_duration_arg(&raw), None);
            }
        }
    }
}
