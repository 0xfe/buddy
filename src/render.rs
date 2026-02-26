//! Backward-compatible renderer exports.
//!
//! The terminal UI implementation now lives under `crate::tui`.

pub use crate::tui::progress::{ProgressHandle, ProgressMetrics};
pub use crate::tui::renderer::Renderer;
