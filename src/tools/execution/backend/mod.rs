//! Backend-specific execution trait implementations.
//!
//! Each module implements the shared contracts for one transport domain.

pub(super) mod container;
pub(super) mod local;
pub(super) mod ssh;
