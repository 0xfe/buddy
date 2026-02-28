//! Binary-local application orchestration helpers.
//!
//! The main binary keeps wiring logic in `main.rs`, while this module hosts
//! reusable command/render helpers to keep entrypoint code small.

pub(crate) mod approval;
pub(crate) mod commands;
pub(crate) mod entry;
pub(crate) mod exec_mode;
pub(crate) mod repl_loop;
pub(crate) mod repl_mode;
pub(crate) mod startup;
pub(crate) mod tasks;

pub(crate) use entry::run;
