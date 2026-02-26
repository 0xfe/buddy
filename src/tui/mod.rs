//! Terminal user-interface building blocks.
//!
//! This module hosts the REPL editor, slash-command parsing, and terminal
//! renderer primitives. The split keeps stateful input logic, layout math,
//! and output styling decoupled so the interface can grow without centralizing
//! unrelated concerns in one file.

pub mod commands;
mod highlight;
pub mod input;
mod input_buffer;
mod input_layout;
mod markdown;
pub mod progress;
mod prompt;
pub mod renderer;
pub mod settings;
pub mod text;

pub use commands::{
    matching_slash_commands, parse_slash_command, SlashCommand, SlashCommandAction, SLASH_COMMANDS,
};
pub use input::{read_repl_line_with_interrupt, ReadOutcome, ReadPoll, ReplState};
pub use prompt::{ApprovalPrompt, PromptMode};
pub use renderer::Renderer;
