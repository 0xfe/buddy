//! Unified terminal-facing UI facade.
//!
//! This module groups rendering contracts, terminal input/output helpers, and
//! runtime-event rendering adapters under one namespace so orchestration layers
//! can depend on `ui` instead of importing many disparate modules.

pub mod render;
pub mod runtime;
pub mod terminal;
pub mod theme;
