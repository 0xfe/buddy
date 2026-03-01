//! Buddy — an agentic AI loop for OpenAI-compatible APIs.
//!
//! This crate provides a configurable AI agent that can hold conversations,
//! invoke tools, and manage context windows. It works with any OpenAI
//! API-compatible endpoint (OpenAI, Ollama, OpenRouter, etc.).
//!
//! # Quick start
//!
//! ```no_run
//! use buddy::agent::Agent;
//! use buddy::config::load_config;
//! use buddy::tools::ToolRegistry;
//!
//! # async fn example() {
//! let config = load_config(None).unwrap();
//! let tools = ToolRegistry::new();
//! let mut agent = Agent::new(config, tools);
//! let response = agent.send("Hello!").await.unwrap();
//! println!("{response}");
//! # }
//! ```

/// Core agent loop and orchestration primitives.
pub mod agent;
/// OpenAI-compatible HTTP client wrappers.
pub mod api;
/// Authentication and login-token helpers.
pub mod auth;
/// Compile-time build/version metadata.
pub mod build_info;
/// Config loading, defaults, and profile selection.
pub mod config;
/// Error types used across crate modules.
pub mod error;
/// Startup validation checks.
pub mod preflight;
/// System prompt rendering utilities.
pub mod prompt;
/// REPL state and command helper logic.
pub mod repl;
/// Runtime actor/event protocol.
pub mod runtime;
/// Session persistence and loading.
pub mod session;
#[cfg(test)]
/// Shared testing utilities compiled only for tests.
pub mod testsupport;
/// Shared text formatting helpers.
pub mod textutil;
/// tmux helper integration.
pub mod tmux;
/// Token estimation and tracking.
pub mod tokens;
/// Built-in tool implementations and registry.
pub mod tools;
/// Terminal UI primitives.
pub mod tui;
/// API model types for chat/completions payloads.
pub mod types;
/// UI facade traits and runtime render adapters.
pub mod ui;
