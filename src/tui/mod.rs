//! Backward-compatible terminal-UI namespace.
//!
//! The canonical terminal UI implementation lives in [`crate::ui::terminal`].
//! This module is a compatibility re-export so existing `buddy::tui::*` users
//! keep working while internal code is unified under `ui`.

pub use crate::ui::terminal::{
    matching_slash_commands, parse_slash_command, pick_from_list, read_repl_line_with_interrupt,
    ApprovalPrompt, PromptMode, ReadOutcome, ReadPoll, Renderer, ReplState, SlashCommand,
    SlashCommandAction, SLASH_COMMANDS,
};

/// Backward-compatible re-export of terminal command metadata.
pub mod commands {
    pub use crate::ui::terminal::commands::*;
}

/// Backward-compatible re-export of progress/spinner primitives.
pub mod progress {
    pub use crate::ui::terminal::progress::*;
}

/// Backward-compatible re-export of interactive input helpers.
pub mod input {
    pub use crate::ui::terminal::input::*;
}

/// Backward-compatible re-export of renderer implementation.
pub mod renderer {
    pub use crate::ui::terminal::renderer::*;
}

/// Backward-compatible re-export of terminal settings constants.
pub mod settings {
    pub use crate::ui::terminal::settings::*;
}

/// Backward-compatible re-export of text helpers.
pub mod text {
    pub use crate::ui::terminal::text::*;
}
