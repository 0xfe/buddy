//! Prompt augmentation helpers.
//!
//! This module keeps dynamic system-prompt enrichment logic isolated from the
//! main request/tool loop (for example, tmux screenshot capture injection).

use super::Agent;
use crate::types::{Message, Role};
use serde_json::Value;

/// Maximum number of characters copied from a captured tmux pane snapshot.
const MAX_TMUX_SCREENSHOT_CHARS: usize = 2_500;

impl Agent {
    /// Update the primary system message with a fresh tmux screenshot block.
    ///
    /// This keeps screenshot context current on every request while replacing
    /// the previous snapshot in-place so history does not accumulate stale
    /// screenshots.
    pub(super) async fn refresh_dynamic_tmux_snapshot_prompt(&mut self) {
        let base = self.config.agent.system_prompt.trim();
        if base.is_empty() {
            return;
        }

        if !should_include_default_tmux_snapshot(&self.messages) {
            set_primary_system_message(&mut self.messages, base.to_string());
            return;
        }

        let Some(snapshot_block) = self.capture_tmux_snapshot_prompt_block().await else {
            set_primary_system_message(&mut self.messages, base.to_string());
            return;
        };
        set_primary_system_message(&mut self.messages, format!("{base}\n\n{snapshot_block}"));
    }

    /// Capture a tmux snapshot and render it into a system-prompt section.
    async fn capture_tmux_snapshot_prompt_block(&self) -> Option<String> {
        if !self.tools.has_tool("capture-pane") {
            return None;
        }

        let result = self.tools.execute("capture-pane", "{}").await.ok()?;
        let snapshot = tool_result_text(&result).trim().to_string();
        if snapshot.is_empty() {
            return None;
        }
        Some(render_tmux_snapshot_block(&snapshot))
    }
}

/// Replace the first system message (or insert one) with updated content.
fn set_primary_system_message(messages: &mut Vec<Message>, content: String) {
    if let Some(first) = messages.first_mut() {
        if first.role == Role::System {
            *first = Message::system(content);
            return;
        }
    }
    messages.insert(0, Message::system(content));
}

/// Extract human-usable tool text from either JSON envelope or raw output.
fn tool_result_text(raw: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return raw.to_string();
    };
    let Some(payload) = value.as_object().and_then(|obj| obj.get("result")).cloned() else {
        return raw.to_string();
    };
    if let Some(text) = payload.as_str() {
        text.to_string()
    } else {
        payload.to_string()
    }
}

/// Render a stable system-prompt block from the current tmux pane snapshot.
fn render_tmux_snapshot_block(snapshot: &str) -> String {
    let mut clipped: String = snapshot.chars().take(MAX_TMUX_SCREENSHOT_CHARS).collect();
    if snapshot.chars().count() > MAX_TMUX_SCREENSHOT_CHARS {
        clipped.push_str("\n...[truncated]");
    }
    format!(
        "Current DEFAULT SHARED PANE screenshot (captured immediately before this request):\n\
```text\n{clipped}\n```\n\
Before running any command, inspect this default shared-pane screenshot. If it does not show a usable shell prompt, \
do not run commands yet. Tell the user what is blocking the pane and offer to recover control with `send-keys`.\n\
If you recently targeted a different tmux pane/session, do not assume this default screenshot represents that target."
    )
}

/// Decide whether the default shared-pane snapshot should be injected.
///
/// If the latest tmux-targeted tool call explicitly selected a non-default
/// target, we omit default shared-pane screenshot injection for this request.
fn should_include_default_tmux_snapshot(messages: &[Message]) -> bool {
    messages
        .iter()
        .rev()
        .find_map(|message| {
            message.tool_calls.as_ref().and_then(|calls| {
                calls
                    .iter()
                    .rev()
                    .find_map(|call| match call.function.name.as_str() {
                        "capture-pane" | "send-keys" | "run_shell" => {
                            Some(!tool_call_targets_non_default(&call.function.arguments))
                        }
                        _ => None,
                    })
            })
        })
        .unwrap_or(true)
}

/// True when tool arguments explicitly target a non-default tmux location.
fn tool_call_targets_non_default(arguments_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(arguments_json) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let has_target = object
        .get("target")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if has_target {
        return true;
    }
    let has_session = object
        .get("session")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty());
    if has_session {
        return true;
    }
    object
        .get("pane")
        .and_then(Value::as_str)
        .is_some_and(|text| {
            let pane = text.trim();
            !pane.is_empty() && pane != "shared"
        })
}

#[cfg(test)]
mod tests {
    use super::{
        render_tmux_snapshot_block, should_include_default_tmux_snapshot,
        tool_call_targets_non_default, tool_result_text,
    };
    use crate::types::{FunctionCall, Message, Role, ToolCall};
    use std::collections::BTreeMap;

    /// Verifies JSON tool envelopes prefer the `result` field text.
    #[test]
    fn tool_result_text_prefers_result_field() {
        let raw = r#"{"result":"hello"}"#;
        assert_eq!(tool_result_text(raw), "hello");
    }

    /// Verifies invalid JSON returns the original raw payload unchanged.
    #[test]
    fn tool_result_text_falls_back_to_raw_payload() {
        let raw = "not json";
        assert_eq!(tool_result_text(raw), "not json");
    }

    /// Verifies oversized snapshots are clipped and marked as truncated.
    #[test]
    fn tmux_snapshot_block_truncates_large_snapshots() {
        let text = "x".repeat(3_000);
        let rendered = render_tmux_snapshot_block(&text);
        assert!(rendered.contains("...[truncated]"));
    }

    /// Verifies explicit pane targeting disables default screenshot injection.
    #[test]
    fn tool_call_target_detection_marks_non_default() {
        assert!(tool_call_targets_non_default(
            r#"{"session":"build","pane":"worker"}"#
        ));
        assert!(!tool_call_targets_non_default(
            r#"{"pane":"shared","risk":"low"}"#
        ));
    }

    /// Verifies snapshot injection is suppressed after non-default tmux calls.
    #[test]
    fn snapshot_injection_suppressed_for_non_default_tmux_target() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: Some(String::new()),
            tool_calls: Some(vec![ToolCall {
                id: "call-1".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "capture-pane".to_string(),
                    arguments: r#"{"session":"build","pane":"worker"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }];
        assert!(!should_include_default_tmux_snapshot(&messages));
    }
}
