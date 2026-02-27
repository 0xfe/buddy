//! HTTP client for OpenAI-compatible APIs.
//!
//! The API layer is split into cohesive protocol modules:
//! - `completions`: `/chat/completions`
//! - `responses`: `/responses`
//! - `policy`: provider-specific transport/runtime rules
//! - `client`: shared auth and dispatch orchestration

mod client;
mod completions;
mod policy;
mod responses;

pub use client::ApiClient;
