//! Terminal primitives re-exported through the `ui` facade.
//!
//! The implementation currently lives in `crate::tui`; this shim keeps callers
//! on a stable `ui::terminal` namespace while internal layout evolves.

pub use crate::tui::settings;
pub use crate::tui::{
    matching_slash_commands, parse_slash_command, pick_from_list, read_repl_line_with_interrupt,
    ApprovalPrompt, PromptMode, ReadOutcome, ReadPoll, Renderer, ReplState, SlashCommand,
    SlashCommandAction, SLASH_COMMANDS,
};
