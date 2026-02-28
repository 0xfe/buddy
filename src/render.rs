//! Backward-compatible rendering exports.
//!
//! Rendering contracts now live in `crate::ui::render`. This module remains as
//! a compatibility shim for callers still importing `crate::render`.

pub use crate::ui::render::*;
