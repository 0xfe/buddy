//! tmux key injection tool.
//!
//! Useful for interactive terminal programs where commands must be controlled
//! with keystrokes (for example Ctrl-C, Ctrl-Z, arrows, Enter).

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::execution::{ExecutionContext, SendKeysOptions};
use super::result_envelope::wrap_result;
use super::shell::RiskLevel;
use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Tool for sending tmux key events to the active pane.
pub struct SendKeysTool {
    /// Where tmux key injection should run (local or SSH with tmux session).
    pub execution: ExecutionContext,
}

#[derive(Deserialize)]
struct Args {
    /// Optional explicit tmux target (`tmux -t` syntax).
    target: Option<String>,
    /// Optional managed tmux session selector.
    session: Option<String>,
    /// Optional managed tmux pane selector.
    pane: Option<String>,
    /// Optional list of tmux key names to send.
    keys: Option<Vec<String>>,
    /// Optional literal text payload (`tmux send-keys -l`).
    literal_text: Option<String>,
    /// Whether to press Enter after the key/text payload.
    enter: Option<bool>,
    /// Optional delay string before key injection.
    delay: Option<String>,
    /// Declared risk classification for this action.
    risk: RiskLevel,
    /// Whether action mutates state.
    mutation: bool,
    /// Whether action involves privilege escalation.
    privesc: bool,
    /// Human rationale for why keys are being sent.
    why: String,
}

#[async_trait]
impl Tool for SendKeysTool {
    fn name(&self) -> &'static str {
        "send-keys"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: concat!(
                    "Send keys directly to a tmux pane to control interactive/stuck terminal programs.\n",
                    "When to use:\n",
                    "- Interrupting, confirming, or navigating an existing interactive process.\n",
                    "- Sending Ctrl-C/Ctrl-Z/Enter/arrows to an already-running program.\n",
                    "When NOT to use:\n",
                    "- Starting new non-interactive commands (use run_shell).\n",
                    "- Reading pane output (use capture-pane).\n",
                    "- Managing tmux structure (use tmux-create/kill tools).\n",
                    "Disambiguation:\n",
                    "- send-keys controls current pane state.\n",
                    "- run_shell launches commands.\n",
                    "- capture-pane observes output.\n",
                    "Examples:\n",
                    "- {\"keys\":[\"C-c\"],\"risk\":\"low\",\"mutation\":false,\"privesc\":false,\"why\":\"Stop hung process\"}\n",
                    "- {\"literal_text\":\"q\",\"enter\":true,\"risk\":\"low\",\"mutation\":false,\"privesc\":false,\"why\":\"Exit pager\"}"
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
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional tmux key names to send (examples: \"C-c\", \"C-z\", \"Enter\", \"Up\", \"Down\")."
                        },
                        "literal_text": {
                            "type": "string",
                            "description": "Optional literal text to type into the pane (tmux send-keys -l)."
                        },
                        "enter": {
                            "type": "boolean",
                            "description": "If true, also press Enter after other key injections."
                        },
                        "delay": {
                            "type": "string",
                            "description": "Optional delay before sending keys, like '500ms', '2s', '1m', or '1h'."
                        },
                        "risk": {
                            "type": "string",
                            "enum": ["low", "medium", "high"],
                            "description": "Estimated risk level for this key injection."
                        },
                        "mutation": {
                            "type": "boolean",
                            "description": "True when this key injection mutates system state."
                        },
                        "privesc": {
                            "type": "boolean",
                            "description": "True when the action uses privilege escalation."
                        },
                        "why": {
                            "type": "string",
                            "description": "Short reason for sending these keys, including risk/privesc justification."
                        }
                    },
                    "required": ["risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        // Parse and validate metadata that accompanies key injection.
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        if args.why.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "send-keys.why must be a non-empty string".to_string(),
            ));
        }
        // Risk metadata is currently validated-for-presence and forwarded to policy layers.
        let _ = (args.risk, args.mutation, args.privesc);
        let delay = resolve_delay(args.delay.as_deref())?;
        // Convert tool args into backend-agnostic execution options.
        let options = SendKeysOptions {
            target: args.target,
            session: args.session,
            pane: args.pane,
            keys: args.keys.unwrap_or_default(),
            literal_text: args.literal_text,
            press_enter: args.enter.unwrap_or(false),
            delay,
        };
        wrap_result(self.execution.send_keys(options).await?)
    }
}

fn resolve_delay(delay: Option<&str>) -> Result<Duration, ToolError> {
    if let Some(raw) = delay {
        return parse_delay_duration(raw).map_err(ToolError::InvalidArguments);
    }
    Ok(Duration::ZERO)
}

fn parse_delay_duration(raw: &str) -> Result<Duration, String> {
    // Shared parser semantics with capture-pane delay handling.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_send_keys() {
        // Tool name must match the registered function name.
        assert_eq!(
            SendKeysTool {
                execution: ExecutionContext::local(),
            }
            .name(),
            "send-keys"
        );
    }

    #[test]
    fn parse_delay_supports_units() {
        // Delay parser should handle milliseconds and minute units.
        assert_eq!(
            parse_delay_duration("500ms").expect("parse"),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_delay_duration("2m").expect("parse"),
            Duration::from_secs(120)
        );
    }

    #[tokio::test]
    async fn execute_missing_required_metadata_returns_error() {
        // Metadata fields are required even when only sending keys.
        let err = SendKeysTool {
            execution: ExecutionContext::local(),
        }
        .execute(r#"{"keys":["C-c"]}"#, &ToolContext::empty())
        .await
        .expect_err("missing metadata should fail");
        assert!(err.to_string().contains("invalid arguments"));
    }

    #[test]
    fn definition_description_contains_guidance_sections() {
        // Description should include structured usage guidance and examples.
        let definition = SendKeysTool {
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
