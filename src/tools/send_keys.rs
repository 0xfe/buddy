//! tmux key injection tool.
//!
//! Useful for interactive terminal programs where commands must be controlled
//! with keystrokes (for example Ctrl-C, Ctrl-Z, arrows, Enter).

use async_trait::async_trait;
use serde::Deserialize;
use std::time::Duration;

use super::execution::{ExecutionContext, SendKeysOptions};
use super::Tool;
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Tool for sending tmux key events to the active pane.
pub struct SendKeysTool {
    /// Where tmux key injection should run (local or SSH with tmux session).
    pub execution: ExecutionContext,
}

#[derive(Deserialize)]
struct Args {
    target: Option<String>,
    keys: Option<Vec<String>>,
    literal_text: Option<String>,
    enter: Option<bool>,
    delay: Option<String>,
    delay_ms: Option<u64>,
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
                description: "Send keys directly to a tmux pane (for example Ctrl-C/Ctrl-Z/Enter/arrows) to control interactive or stuck terminal programs. Common flow: run_shell with wait=false, poll with capture-pane, and send keys as needed."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Optional tmux target pane/session (same syntax as tmux -t). Defaults to the active agent pane."
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
                        "delay_ms": {
                            "type": "integer",
                            "minimum": 0,
                            "description": "Optional delay before sending keys in milliseconds."
                        }
                    }
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String, ToolError> {
        let args: Args = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let delay = resolve_delay(args.delay.as_deref(), args.delay_ms)?;
        let options = SendKeysOptions {
            target: args.target,
            keys: args.keys.unwrap_or_default(),
            literal_text: args.literal_text,
            press_enter: args.enter.unwrap_or(false),
            delay,
        };
        self.execution.send_keys(options).await
    }
}

fn resolve_delay(delay: Option<&str>, delay_ms: Option<u64>) -> Result<Duration, ToolError> {
    if delay.is_some() && delay_ms.is_some() {
        return Err(ToolError::InvalidArguments(
            "provide either `delay` or `delay_ms`, not both".into(),
        ));
    }
    if let Some(ms) = delay_ms {
        return Ok(Duration::from_millis(ms));
    }
    if let Some(raw) = delay {
        return parse_delay_duration(raw).map_err(ToolError::InvalidArguments);
    }
    Ok(Duration::ZERO)
}

fn parse_delay_duration(raw: &str) -> Result<Duration, String> {
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
        assert_eq!(
            parse_delay_duration("500ms").expect("parse"),
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_delay_duration("2m").expect("parse"),
            Duration::from_secs(120)
        );
    }
}
