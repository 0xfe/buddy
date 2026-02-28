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

pub mod agent;
pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod preflight;
pub mod prompt;
pub mod repl;
pub mod render;
pub mod ui;
pub mod runtime;
pub mod session;
#[cfg(test)]
pub mod testsupport;
pub mod textutil;
pub mod tokens;
pub mod tools;
pub mod tui;
pub mod types;
