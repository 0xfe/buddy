//! Rendering contracts and default terminal renderer bindings.
//!
//! `RenderSink` is the UI contract consumed by orchestration layers. Keeping it
//! under `ui` decouples runtime/app logic from a specific renderer module path.

pub use crate::tui::progress::{ProgressHandle, ProgressMetrics};
pub use crate::tui::renderer::Renderer;

/// Injectable rendering interface used by orchestration code.
///
/// `Renderer` remains the default terminal implementation, but consumers/tests
/// can substitute a mock sink without coupling to stderr output.
pub trait RenderSink: Send + Sync {
    /// Render the interactive prompt chrome.
    fn prompt(&self);
    /// Render one assistant message destined for stdout.
    fn assistant_message(&self, content: &str);
    /// Start a progress indicator for a long-running task.
    fn progress(&self, label: &str) -> ProgressHandle;
    /// Start a progress indicator enriched with metrics.
    fn progress_with_metrics(&self, label: &str, metrics: ProgressMetrics) -> ProgressHandle;
    /// Render a session/model header line.
    fn header(&self, model: &str);
    /// Render a tool invocation summary.
    fn tool_call(&self, name: &str, args: &str);
    /// Render a tool result summary.
    fn tool_result(&self, result: &str);
    /// Render token usage counters.
    fn token_usage(&self, prompt: u64, completion: u64, session_total: u64);
    /// Render reasoning trace fragments with a field label.
    fn reasoning_trace(&self, field: &str, trace: &str);
    /// Render a warning line.
    fn warn(&self, msg: &str);
    /// Render a titled section divider.
    fn section(&self, title: &str);
    /// Render activity/lifecycle text.
    fn activity(&self, text: &str);
    /// Render one key/value field row.
    fn field(&self, key: &str, value: &str);
    /// Render additional detail text.
    fn detail(&self, text: &str);
    /// Render an error line.
    fn error(&self, msg: &str);
    /// Render syntax-aware tool output as a block.
    fn tool_output_block(&self, text: &str, syntax_path: Option<&str>);
    /// Render raw command output as a block.
    fn command_output_block(&self, text: &str);
    /// Render model reasoning text as a block.
    fn reasoning_block(&self, text: &str);
    /// Render approval-related text as a block.
    fn approval_block(&self, text: &str);
}

impl RenderSink for Renderer {
    fn prompt(&self) {
        self.prompt();
    }

    fn assistant_message(&self, content: &str) {
        self.assistant_message(content);
    }

    fn progress(&self, label: &str) -> ProgressHandle {
        self.progress(label)
    }

    fn progress_with_metrics(&self, label: &str, metrics: ProgressMetrics) -> ProgressHandle {
        self.progress_with_metrics(label, metrics)
    }

    fn header(&self, model: &str) {
        self.header(model);
    }

    fn tool_call(&self, name: &str, args: &str) {
        self.tool_call(name, args);
    }

    fn tool_result(&self, result: &str) {
        self.tool_result(result);
    }

    fn token_usage(&self, prompt: u64, completion: u64, session_total: u64) {
        self.token_usage(prompt, completion, session_total);
    }

    fn reasoning_trace(&self, field: &str, trace: &str) {
        self.reasoning_trace(field, trace);
    }

    fn warn(&self, msg: &str) {
        self.warn(msg);
    }

    fn section(&self, title: &str) {
        self.section(title);
    }

    fn activity(&self, text: &str) {
        self.activity(text);
    }

    fn field(&self, key: &str, value: &str) {
        self.field(key, value);
    }

    fn detail(&self, text: &str) {
        self.detail(text);
    }

    fn error(&self, msg: &str) {
        self.error(msg);
    }

    fn tool_output_block(&self, text: &str, syntax_path: Option<&str>) {
        self.tool_output_block(text, syntax_path);
    }

    fn command_output_block(&self, text: &str) {
        self.command_output_block(text);
    }

    fn reasoning_block(&self, text: &str) {
        self.reasoning_block(text);
    }

    fn approval_block(&self, text: &str) {
        self.approval_block(text);
    }
}

/// Global progress toggle helper decoupled from concrete `Renderer` type.
pub fn set_progress_enabled(enabled: bool) {
    Renderer::set_progress_enabled(enabled);
}
