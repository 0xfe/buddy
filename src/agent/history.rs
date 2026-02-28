//! Conversation history compaction.
//!
//! The agent uses this module to collapse older turns into a compact system
//! summary when context pressure is high or when `/session compact` is invoked.

use super::Agent;
use crate::tokens::TokenTracker;
use crate::types::{Message, Role};

/// Minimum number of most-recent turns preserved during compaction.
const CONTEXT_COMPACT_KEEP_RECENT_TURNS: usize = 3;
/// Max number of summary lines generated from removed messages.
const MAX_COMPACT_SUMMARY_LINES: usize = 24;
/// Prefix marker used to identify synthetic compaction summary messages.
pub(super) const COMPACT_SUMMARY_PREFIX: &str = "[buddy compact summary]";

/// Details about one history-compaction operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCompactionReport {
    /// Estimated token count before compaction.
    pub estimated_before: u64,
    /// Estimated token count after compaction.
    pub estimated_after: u64,
    /// Number of individual messages removed from history.
    pub removed_messages: usize,
    /// Number of turn groups removed from history.
    pub removed_turns: usize,
}

impl Agent {
    /// Compact older conversation turns into a synthesized summary message.
    ///
    /// This is used by `/compact` and can also be triggered automatically
    /// before request submission when context pressure is high.
    pub fn compact_history(&mut self) -> Option<HistoryCompactionReport> {
        compact_history_with_budget(
            &mut self.messages,
            self.tracker.context_limit,
            super::CONTEXT_MANUAL_COMPACT_TARGET_FRACTION,
            true,
        )
    }
}

/// Compact message history to a target context fraction.
///
/// `force=true` removes old turns aggressively (used for manual compaction).
/// `force=false` compacts only when current estimate exceeds `target_fraction`.
pub(super) fn compact_history_with_budget(
    messages: &mut Vec<Message>,
    context_limit: usize,
    target_fraction: f64,
    force: bool,
) -> Option<HistoryCompactionReport> {
    // Guard rails: if we cannot estimate a useful budget, skip compaction.
    if context_limit == 0 || messages.is_empty() {
        return None;
    }

    let estimated_before = TokenTracker::estimate_messages(messages);
    let target_tokens = ((context_limit as f64) * target_fraction).floor().max(1.0) as usize;
    if !force && estimated_before <= target_tokens {
        return None;
    }

    // Reuse a previous compact summary if one is already present directly
    // after leading system prompts. This prevents stacked summary messages.
    let mut insertion_index = leading_system_count(messages);
    let mut previous_summary = None;
    if insertion_index > 0
        && messages
            .get(insertion_index - 1)
            .is_some_and(is_compact_summary_message)
    {
        if let Some(removed) = messages.get(insertion_index - 1).cloned() {
            previous_summary = removed.content;
        }
        messages.remove(insertion_index - 1);
        insertion_index -= 1;
    }

    let mut removed_messages = Vec::new();
    let mut removed_turns = 0usize;

    loop {
        let estimated_now = TokenTracker::estimate_messages(messages);
        let turns = collect_turn_ranges(messages, insertion_index);
        if turns.len() <= CONTEXT_COMPACT_KEEP_RECENT_TURNS {
            break;
        }

        // Forced mode always trims until only the recent window remains.
        // Automatic mode trims only while usage exceeds budget.
        let should_remove = if force {
            estimated_now > target_tokens || turns.len() > CONTEXT_COMPACT_KEEP_RECENT_TURNS + 1
        } else {
            estimated_now > target_tokens
        };
        if !should_remove {
            break;
        }

        let turn = turns[0];
        removed_messages.extend(messages.drain(turn.start..turn.end));
        removed_turns = removed_turns.saturating_add(1);
    }

    if removed_messages.is_empty() && previous_summary.is_none() {
        return None;
    }

    let summary = build_compact_summary(previous_summary.as_deref(), &removed_messages);
    messages.insert(insertion_index, Message::system(summary));

    let mut estimated_after = TokenTracker::estimate_messages(messages);
    // If the generated summary does not reduce estimated size, fall back to a
    // minimal summary and then remove it entirely if still not helpful.
    if estimated_after >= estimated_before {
        messages[insertion_index] = Message::system(format!(
            "{COMPACT_SUMMARY_PREFIX}\nOlder turns were compacted."
        ));
        estimated_after = TokenTracker::estimate_messages(messages);
        if estimated_after >= estimated_before {
            messages.remove(insertion_index);
            estimated_after = TokenTracker::estimate_messages(messages);
        }
    }

    Some(HistoryCompactionReport {
        estimated_before: estimated_before as u64,
        estimated_after: estimated_after as u64,
        removed_messages: removed_messages.len(),
        removed_turns,
    })
}

/// Half-open index range for one conversation turn in `messages`.
#[derive(Clone, Copy)]
struct TurnRange {
    /// Inclusive start index of the turn.
    start: usize,
    /// Exclusive end index of the turn.
    end: usize,
}

/// Count contiguous leading `system` messages.
fn leading_system_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .take_while(|message| message.role == Role::System)
        .count()
}

/// Split message history into turn ranges starting from `start_index`.
fn collect_turn_ranges(messages: &[Message], start_index: usize) -> Vec<TurnRange> {
    let mut turns = Vec::new();
    let mut current_start: Option<usize> = None;

    for (idx, message) in messages.iter().enumerate().skip(start_index) {
        if message.role == Role::User {
            if let Some(start) = current_start {
                turns.push(TurnRange { start, end: idx });
            }
            current_start = Some(idx);
        } else if current_start.is_none() {
            current_start = Some(idx);
        }
    }

    if let Some(start) = current_start {
        turns.push(TurnRange {
            start,
            end: messages.len(),
        });
    }

    turns
}

/// Return true if message is a synthetic compaction summary.
fn is_compact_summary_message(message: &Message) -> bool {
    message.role == Role::System
        && message
            .content
            .as_deref()
            .is_some_and(|text| text.starts_with(COMPACT_SUMMARY_PREFIX))
}

/// Build a new summary text from the removed messages and prior summary body.
fn build_compact_summary(previous_summary: Option<&str>, removed_messages: &[Message]) -> String {
    let mut lines = Vec::new();
    lines.push(COMPACT_SUMMARY_PREFIX.to_string());
    lines.push("Older turns were compacted to preserve room for newer context.".to_string());

    if let Some(previous) = previous_summary.and_then(compact_summary_body) {
        if !previous.is_empty() {
            lines.push(format!("Previously compacted summary: {previous}"));
        }
    }

    let mut added = 0usize;
    for message in removed_messages {
        if added >= MAX_COMPACT_SUMMARY_LINES {
            break;
        }
        if let Some(line) = compact_message_line(message) {
            lines.push(line);
            added += 1;
        }
    }

    if removed_messages.len() > added {
        lines.push(format!(
            "... {} additional compacted message(s) omitted",
            removed_messages.len() - added
        ));
    }

    lines.join("\n")
}

/// Extract and normalize the body section from a prior summary message.
fn compact_summary_body(summary: &str) -> Option<String> {
    let mut lines = summary.lines();
    let first = lines.next()?.trim();
    if first != COMPACT_SUMMARY_PREFIX {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join(" ");
    let body = body.trim();
    if body.is_empty() {
        None
    } else {
        Some(truncate_summary_preview(body))
    }
}

/// Render one removed message into a compact one-line summary entry.
fn compact_message_line(message: &Message) -> Option<String> {
    match message.role {
        Role::System => None,
        Role::User => message
            .content
            .as_deref()
            .map(|text| format!("user: {}", truncate_summary_preview(text))),
        Role::Assistant => {
            let mut parts = Vec::new();
            if let Some(content) = message.content.as_deref().map(str::trim) {
                if !content.is_empty() {
                    parts.push(format!("assistant: {}", truncate_summary_preview(content)));
                }
            }
            if let Some(tool_calls) = &message.tool_calls {
                let names = tool_calls
                    .iter()
                    .map(|call| call.function.name.as_str())
                    .collect::<Vec<_>>();
                if !names.is_empty() {
                    parts.push(format!(
                        "assistant tools: {}",
                        truncate_summary_preview(&names.join(", "))
                    ));
                }
            }
            (!parts.is_empty()).then(|| parts.join(" | "))
        }
        Role::Tool => {
            let id = message.tool_call_id.as_deref().unwrap_or("<unknown>");
            let content = message.content.as_deref().unwrap_or("");
            Some(format!(
                "tool ({id}): {}",
                truncate_summary_preview(content)
            ))
        }
    }
}

/// Clip summary preview text so compaction output stays bounded.
fn truncate_summary_preview(text: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 180;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_PREVIEW_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed
        .chars()
        .take(MAX_PREVIEW_CHARS.saturating_sub(3))
        .collect::<String>();
    format!("{prefix}...")
}
