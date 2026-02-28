//! Binary-local application orchestration helpers.
//!
//! The main binary keeps wiring logic in `main.rs`, while this module hosts
//! reusable command/render helpers to keep entrypoint code small.

/// Approval prompt and decision helpers.
pub(crate) mod approval;
/// Slash-command helper modules.
pub(crate) mod commands;
/// Main application entry orchestration.
pub(crate) mod entry;
/// One-shot exec mode orchestration.
pub(crate) mod exec_mode;
/// Shared slash-command dispatch for REPL/approval prompts.
pub(crate) mod repl_loop;
/// Interactive REPL mode orchestration.
pub(crate) mod repl_mode;
/// Startup banner/session status helpers.
pub(crate) mod startup;
/// Background-task and runtime-event state helpers.
pub(crate) mod tasks;

/// Binary entrypoint used by `main`.
pub(crate) use entry::run;
