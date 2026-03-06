//! tmux pane capture tool.
//!
//! This tool is intended for interactive workflows where command output is
//! still evolving on screen (for example full-screen apps or stuck commands).

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::execution::{CapturePaneOptions, ExecutionContext};
use super::result_envelope::wrap_result;
use super::{require_tool_why, Tool, ToolContext};
use crate::error::ToolError;
use crate::textutil::safe_prefix_by_bytes;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Maximum characters returned to the model from one pane capture.
const MAX_CAPTURE_LEN: usize = 8000;

/// Tool that captures a tmux pane snapshot.
pub struct CapturePaneTool {
    /// Where tmux capture should run (local or SSH with tmux session).
    pub execution: ExecutionContext,
}

#[derive(Deserialize)]
struct Args {
    /// Optional explicit pane/session target (`tmux -t` syntax).
    target: Option<String>,
    /// Optional managed session selector.
    session: Option<String>,
    /// Optional managed pane selector.
    pane: Option<String>,
    /// Optional tmux `-S` capture start boundary.
    start: Option<String>,
    /// Optional tmux `-E` capture end boundary.
    end: Option<String>,
    /// Whether wrapped lines should be joined (`tmux -J`).
    #[serde(default = "default_join_wrapped_lines")]
    join_wrapped_lines: bool,
    /// Whether trailing spaces should be preserved (`tmux -N`).
    #[serde(default)]
    preserve_trailing_spaces: bool,
    /// Whether escape sequences should be included (`tmux -e`).
    #[serde(default)]
    include_escape_sequences: bool,
    /// Whether non-printables should be escaped (`tmux -C`).
    #[serde(default)]
    escape_non_printable: bool,
    /// Whether alternate screen should be captured (`tmux -a`).
    #[serde(default)]
    include_alternate_screen: bool,
    /// Optional string duration before capture.
    delay: Option<String>,
    /// Human rationale for capturing pane output now.
    why: String,
}

fn default_join_wrapped_lines() -> bool {
    // Most CLI output is easier for the model to parse when wraps are joined.
    true
}

#[async_trait]
impl Tool for CapturePaneTool {
    fn name(&self) -> &'static str {
        "tmux_capture_pane"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: concat!(
                    "Capture tmux pane output (default: visible screenshot range).\n",
                    "When to use:\n",
                    "- Polling output for long-running commands started with run_shell wait=false.\n",
                    "- Inspecting interactive/stuck terminal state before deciding next action.\n",
                    "When NOT to use:\n",
                    "- Executing commands (use run_shell).\n",
                    "- Sending control input (use tmux_send_keys).\n",
                    "Disambiguation:\n",
                    "- tmux_capture_pane reads pane state only.\n",
                    "- run_shell executes commands.\n",
                    "- tmux_send_keys changes interactive program state.\n",
                    "Examples:\n",
                    "- {\"delay\":\"2s\",\"why\":\"Poll the shared pane for output from a background command.\"}\n",
                    "- {\"session\":\"build\",\"pane\":\"worker\",\"start\":\"-200\",\"end\":\"-\",\"why\":\"Inspect the build worker pane before deciding the next action.\"}"
                ).into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Optional explicit tmux `-t` target (for example `%7` or `session:window.pane`). Legacy escape hatch; usually omit this and use managed `session`/`pane` (or neither for default shared pane)."
                        },
                        "session": {
                            "type": "string",
                            "description": "Optional managed tmux session selector. Usually omit this to use the default shared session."
                        },
                        "pane": {
                            "type": "string",
                            "description": "Optional managed tmux pane selector. Usually omit this to use the default shared pane."
                        },
                        "start": {
                            "type": "string",
                            "description": "Optional tmux -S start line (for example '-', '-200', or '0'). If omitted, tmux default screenshot range is used."
                        },
                        "end": {
                            "type": "string",
                            "description": "Optional tmux -E end line (for example '-', '0', or '200'). If omitted, tmux default screenshot range is used."
                        },
                        "join_wrapped_lines": {
                            "type": "boolean",
                            "description": "Include tmux -J to join wrapped lines. Defaults to true."
                        },
                        "preserve_trailing_spaces": {
                            "type": "boolean",
                            "description": "Include tmux -N to preserve trailing spaces."
                        },
                        "include_escape_sequences": {
                            "type": "boolean",
                            "description": "Include tmux -e to keep ANSI escape sequences."
                        },
                        "escape_non_printable": {
                            "type": "boolean",
                            "description": "Include tmux -C to octal-escape non-printable characters."
                        },
                        "include_alternate_screen": {
                            "type": "boolean",
                            "description": "Include tmux -a to capture alternate screen content when available."
                        },
                        "delay": {
                            "type": "string",
                            "description": "Optional delay before capture, like '500ms', '2s', '1m', or '1h'. Useful for polling."
                        },
                        "why": {
                            "type": "string",
                            "description": "One or two lines explaining why this pane capture is needed right now."
                        }
                    },
                    "required": ["why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        // Parse capture options and normalize delay controls.
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        require_tool_why(self.name(), &args.why)?;
        let delay = resolve_delay(&args)?;

        // Translate tool JSON args into backend-neutral capture options.
        let mut options = CapturePaneOptions::default();
        if let Some(target) = args.target {
            options.target = Some(target);
        }
        if let Some(session) = args.session {
            options.session = Some(session);
        }
        if let Some(pane) = args.pane {
            options.pane = Some(pane);
        }
        if let Some(start) = args.start {
            options.start = Some(start);
        }
        if let Some(end) = args.end {
            options.end = Some(end);
        }
        options.join_wrapped_lines = args.join_wrapped_lines;
        options.preserve_trailing_spaces = args.preserve_trailing_spaces;
        options.include_escape_sequences = args.include_escape_sequences;
        options.escape_non_printable = args.escape_non_printable;
        options.include_alternate_screen = args.include_alternate_screen;
        options.delay = delay;

        let output = self.execution.capture_pane(options).await?;
        wrap_result(truncate_output_tail(&output, MAX_CAPTURE_LEN))
    }
}

fn resolve_delay(args: &Args) -> Result<Duration, ToolError> {
    if let Some(delay) = args.delay.as_deref() {
        return parse_delay_duration(delay).map_err(ToolError::InvalidArguments);
    }

    Ok(Duration::ZERO)
}

fn parse_delay_duration(raw: &str) -> Result<Duration, String> {
    // Keep parser intentionally strict and deterministic for model-authored inputs.
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("delay cannot be empty".to_string());
    }

    let split = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (num, unit) = trimmed.split_at(split);
    let value = num
        .parse::<u64>()
        .map_err(|_| format!("invalid delay value `{raw}`"))?;
    let unit = unit.trim().to_ascii_lowercase();

    let multiplier = match unit.as_str() {
        "" | "s" => 1000u64,
        "ms" => 1u64,
        "m" => 60_000u64,
        "h" => 3_600_000u64,
        _ => {
            return Err(format!(
                "invalid delay unit `{unit}`; expected ms, s, m, or h"
            ))
        }
    };

    let millis = value
        .checked_mul(multiplier)
        .ok_or_else(|| "delay is too large".to_string())?;
    Ok(Duration::from_millis(millis))
}

fn truncate_output_tail(text: &str, max_len: usize) -> String {
    // Keep latest pane content because recent lines are usually most relevant.
    if text.len() > max_len {
        let skipped = text.len().saturating_sub(max_len);
        let mut start = text.len().saturating_sub(max_len);
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        let suffix = if start >= text.len() {
            // Fall back to a short, safe prefix when the tail boundary cannot be
            // advanced to a valid character boundary.
            safe_prefix_by_bytes(text, max_len)
        } else {
            &text[start..]
        };
        format!("[truncated {skipped} chars from start]\n{}", suffix)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_capture_pane() {
        // Tool name must match the registered function name.
        assert_eq!(
            CapturePaneTool {
                execution: ExecutionContext::local(),
            }
            .name(),
            "tmux_capture_pane"
        );
    }

    #[test]
    fn parse_delay_supports_common_units() {
        // Delay parser should accept standard units used by tool callers.
        assert_eq!(
            parse_delay_duration("10ms").unwrap(),
            Duration::from_millis(10)
        );
        assert_eq!(parse_delay_duration("2").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_delay_duration("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(
            parse_delay_duration("3m").unwrap(),
            Duration::from_secs(180)
        );
        assert_eq!(
            parse_delay_duration("1h").unwrap(),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn parse_delay_rejects_bad_unit() {
        // Unsupported units should return actionable validation errors.
        let err = parse_delay_duration("2d").expect_err("should reject unit");
        assert!(err.contains("invalid delay unit"), "got: {err}");
    }

    #[test]
    fn default_window_uses_tmux_visible_screenshot_range() {
        // Default capture should use tmux's visible-range behavior.
        let opts = CapturePaneOptions::default();
        assert!(opts.start.is_none());
        assert!(opts.end.is_none());
    }

    #[test]
    fn truncate_output_tail_marks_large_capture() {
        // Tail truncation should add metadata and keep latest text.
        let out = truncate_output_tail("x".repeat(MAX_CAPTURE_LEN + 1).as_str(), MAX_CAPTURE_LEN);
        assert!(out.starts_with("[truncated "), "got: {out}");
        assert!(out.ends_with('x'), "got: {out}");
    }

    #[test]
    fn truncate_output_tail_handles_utf8_boundaries() {
        // Tail truncation should not split multibyte UTF-8 characters.
        let input = "🙂".repeat(MAX_CAPTURE_LEN + 2);
        let out = truncate_output_tail(&input, MAX_CAPTURE_LEN + 1);
        assert!(out.starts_with("[truncated "), "got: {out}");
    }

    #[test]
    fn definition_description_contains_guidance_sections() {
        // Description should include structured tool-choice guidance.
        let definition = CapturePaneTool {
            execution: ExecutionContext::local(),
        }
        .definition();
        let description = definition.function.description;
        assert!(description.contains("When to use:"));
        assert!(description.contains("When NOT to use:"));
        assert!(description.contains("Disambiguation:"));
        assert!(description.contains("Examples:"));
    }
}
