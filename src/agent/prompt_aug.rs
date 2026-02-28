//! Prompt augmentation helpers.
//!
//! This module keeps dynamic system-prompt enrichment logic isolated from the
//! main request/tool loop (for example, tmux screenshot capture injection).

use super::Agent;
use crate::types::{Message, Role};
use serde_json::Value;

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

        let Some(snapshot_block) = self.capture_tmux_snapshot_prompt_block().await else {
            set_primary_system_message(&mut self.messages, base.to_string());
            return;
        };
        set_primary_system_message(&mut self.messages, format!("{base}\n\n{snapshot_block}"));
    }

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

fn set_primary_system_message(messages: &mut Vec<Message>, content: String) {
    if let Some(first) = messages.first_mut() {
        if first.role == Role::System {
            *first = Message::system(content);
            return;
        }
    }
    messages.insert(0, Message::system(content));
}

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

fn render_tmux_snapshot_block(snapshot: &str) -> String {
    let mut clipped: String = snapshot.chars().take(MAX_TMUX_SCREENSHOT_CHARS).collect();
    if snapshot.chars().count() > MAX_TMUX_SCREENSHOT_CHARS {
        clipped.push_str("\n...[truncated]");
    }
    format!(
        "Current tmux pane screenshot (captured immediately before this request):\n\
```text\n{clipped}\n```\n\
Before running any command, inspect this screenshot. If it does not show a usable shell prompt, \
do not run commands yet. Tell the user what is blocking the pane and offer to recover control with `send-keys`."
    )
}

#[cfg(test)]
mod tests {
    use super::{render_tmux_snapshot_block, tool_result_text};

    #[test]
    fn tool_result_text_prefers_result_field() {
        let raw = r#"{"result":"hello"}"#;
        assert_eq!(tool_result_text(raw), "hello");
    }

    #[test]
    fn tool_result_text_falls_back_to_raw_payload() {
        let raw = "not json";
        assert_eq!(tool_result_text(raw), "not json");
    }

    #[test]
    fn tmux_snapshot_block_truncates_large_snapshots() {
        let text = "x".repeat(3_000);
        let rendered = render_tmux_snapshot_block(&text);
        assert!(rendered.contains("...[truncated]"));
    }
}
