//! Backward-compatible renderer exports.
//!
//! The terminal UI implementation now lives under `crate::tui`.

pub use crate::tui::progress::{ProgressHandle, ProgressMetrics};
pub use crate::tui::renderer::Renderer;

/// Injectable rendering interface used by orchestration code.
///
/// `Renderer` remains the default terminal implementation, but consumers/tests
/// can now substitute a mock sink without coupling to stderr output.
pub trait RenderSink: Send + Sync {
    fn prompt(&self);
    fn assistant_message(&self, content: &str);
    fn progress(&self, label: &str) -> ProgressHandle;
    fn progress_with_metrics(&self, label: &str, metrics: ProgressMetrics) -> ProgressHandle;
    fn header(&self, model: &str);
    fn tool_call(&self, name: &str, args: &str);
    fn tool_result(&self, result: &str);
    fn token_usage(&self, prompt: u64, completion: u64, session_total: u64);
    fn reasoning_trace(&self, field: &str, trace: &str);
    fn warn(&self, msg: &str);
    fn section(&self, title: &str);
    fn activity(&self, text: &str);
    fn field(&self, key: &str, value: &str);
    fn detail(&self, text: &str);
    fn error(&self, msg: &str);
    fn tool_output_block(&self, text: &str, syntax_path: Option<&str>);
    fn command_output_block(&self, text: &str);
    fn reasoning_block(&self, text: &str);
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
