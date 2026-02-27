//! HTTP client for OpenAI-compatible APIs.
//!
//! The API layer is split into cohesive protocol modules:
//! - `completions`: `/chat/completions`
//! - `responses`: `/responses`
//! - `policy`: provider-specific transport/runtime rules
//! - `client`: shared auth and dispatch orchestration

use crate::error::ApiError;
use crate::types::{ChatRequest, ChatResponse};
use async_trait::async_trait;

mod client;
mod completions;
mod policy;
mod responses;

pub use client::ApiClient;

/// Minimal model API interface used by the agent loop.
///
/// This trait lets tests provide deterministic mock responses without network
/// calls while the production path uses [`ApiClient`].
#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError>;
}
