//! Core agentic loop.
//!
//! The [`Agent`] drives the conversation: it sends user messages to the API,
//! handles tool call responses by executing tools and re-submitting results,
//! and loops until the model produces a final text response (or the iteration
//! cap is reached).

use crate::api::ApiClient;
use crate::config::{ApiConfig, Config};
use crate::error::AgentError;
use crate::render::Renderer;
use crate::tokens::{self, TokenTracker};
use crate::tools::ToolRegistry;
use crate::types::{ChatRequest, Message, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

const CANCELLED_BY_USER_TOOL_RESULT: &str = "operation cancelled by user";
const CANCELLED_BY_USER_PROMPT_RESPONSE: &str = "operation cancelled by user";

/// Persistable conversation + token state for session save/resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionSnapshot {
    pub messages: Vec<Message>,
    pub tracker: TokenTrackerSnapshot,
}

/// Persistable mirror of [`TokenTracker`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTrackerSnapshot {
    pub context_limit: usize,
    pub total_prompt_tokens: u32,
    pub total_completion_tokens: u32,
    pub last_prompt_tokens: u32,
    pub last_completion_tokens: u32,
}

impl TokenTrackerSnapshot {
    fn from_tracker(tracker: &TokenTracker) -> Self {
        Self {
            context_limit: tracker.context_limit,
            total_prompt_tokens: tracker.total_prompt_tokens,
            total_completion_tokens: tracker.total_completion_tokens,
            last_prompt_tokens: tracker.last_prompt_tokens,
            last_completion_tokens: tracker.last_completion_tokens,
        }
    }

    fn into_tracker(self) -> TokenTracker {
        TokenTracker {
            context_limit: self.context_limit,
            total_prompt_tokens: self.total_prompt_tokens,
            total_completion_tokens: self.total_completion_tokens,
            last_prompt_tokens: self.last_prompt_tokens,
            last_completion_tokens: self.last_completion_tokens,
        }
    }
}

/// Background UI events emitted by an agent running in background mode.
#[derive(Debug, Clone)]
pub enum AgentUiEvent {
    Warning {
        task_id: u64,
        message: String,
    },
    TokenUsage {
        task_id: u64,
        prompt_tokens: u32,
        completion_tokens: u32,
        session_total: u32,
    },
    ReasoningTrace {
        task_id: u64,
        field: String,
        trace: String,
    },
    ToolCall {
        task_id: u64,
        name: String,
        args: String,
    },
    ToolResult {
        task_id: u64,
        name: String,
        args: String,
        result: String,
    },
}

/// The core agent that orchestrates the conversation and tool-use loop.
pub struct Agent {
    client: ApiClient,
    config: Config,
    tools: ToolRegistry,
    messages: Vec<Message>,
    tracker: TokenTracker,
    renderer: Renderer,
    suppress_live_output: bool,
    live_output_sink: Option<(u64, mpsc::UnboundedSender<AgentUiEvent>)>,
    cancellation_rx: Option<watch::Receiver<bool>>,
}

impl Agent {
    /// Create an agent from configuration with tools pre-registered.
    pub fn new(config: Config, tools: ToolRegistry) -> Self {
        let client = ApiClient::new(&config.api);
        let context_limit = config
            .api
            .context_limit
            .unwrap_or_else(|| tokens::default_context_limit(&config.api.model));
        let tracker = TokenTracker::new(context_limit);
        let renderer = Renderer::new(config.display.color);
        let messages = initial_messages(&config);

        Self {
            client,
            config,
            tools,
            messages,
            tracker,
            renderer,
            suppress_live_output: false,
            live_output_sink: None,
            cancellation_rx: None,
        }
    }

    /// Replace the active API/model settings without resetting conversation state.
    ///
    /// Used by runtime model switching (`/model`).
    pub fn switch_api_config(&mut self, api: ApiConfig) {
        let context_limit = api
            .context_limit
            .unwrap_or_else(|| tokens::default_context_limit(&api.model));
        self.client = ApiClient::new(&api);
        self.config.api = api;
        self.tracker.context_limit = context_limit;
    }

    /// Snapshot in-memory conversation state for persistent sessions.
    pub fn snapshot_session(&self) -> AgentSessionSnapshot {
        AgentSessionSnapshot {
            messages: self.messages.clone(),
            tracker: TokenTrackerSnapshot::from_tracker(&self.tracker),
        }
    }

    /// Restore conversation/token state from a previously saved snapshot.
    pub fn restore_session(&mut self, snapshot: AgentSessionSnapshot) {
        self.messages = if snapshot.messages.is_empty() {
            initial_messages(&self.config)
        } else {
            snapshot.messages
        };
        self.tracker = snapshot.tracker.into_tracker();
    }

    /// Reset conversation state to a fresh session (keeps model/tools/config).
    pub fn reset_session(&mut self) {
        let context_limit = self.tracker.context_limit;
        self.messages = initial_messages(&self.config);
        self.tracker = TokenTracker::new(context_limit);
    }

    /// Suppress live stderr traces while the agent runs in a background task.
    pub fn set_live_output_suppressed(&mut self, suppressed: bool) {
        self.suppress_live_output = suppressed;
    }

    /// Route live UI events for background-mode rendering in the foreground UI loop.
    pub fn set_live_output_sink(
        &mut self,
        sink: Option<(u64, mpsc::UnboundedSender<AgentUiEvent>)>,
    ) {
        self.live_output_sink = sink;
    }

    /// Register a cancellation signal for the current in-flight request.
    pub fn set_cancellation_receiver(&mut self, rx: Option<watch::Receiver<bool>>) {
        self.cancellation_rx = rx;
    }

    fn cancellation_requested(&self) -> bool {
        self.cancellation_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    fn emit_ui_event(&self, make_event: impl FnOnce(u64) -> AgentUiEvent) -> bool {
        let Some((task_id, tx)) = &self.live_output_sink else {
            return false;
        };
        tx.send(make_event(*task_id)).is_ok()
    }

    fn warn_live(&self, msg: &str) {
        if self.suppress_live_output {
            let sent = self.emit_ui_event(|task_id| AgentUiEvent::Warning {
                task_id,
                message: msg.to_string(),
            });
            if sent {
                return;
            }
        }
        self.renderer.warn(msg);
    }

    fn token_usage_live(&self, prompt: u32, completion: u32, session_total: u32) {
        if self.suppress_live_output {
            let sent = self.emit_ui_event(|task_id| AgentUiEvent::TokenUsage {
                task_id,
                prompt_tokens: prompt,
                completion_tokens: completion,
                session_total,
            });
            if sent {
                return;
            }
        }
        self.renderer.token_usage(prompt, completion, session_total);
    }

    fn reasoning_trace_live(&self, field: &str, trace: &str) {
        if self.suppress_live_output {
            let sent = self.emit_ui_event(|task_id| AgentUiEvent::ReasoningTrace {
                task_id,
                field: field.to_string(),
                trace: trace.to_string(),
            });
            if sent {
                return;
            }
        }
        self.renderer.reasoning_trace(field, trace);
    }

    fn tool_call_live(&self, name: &str, args: &str) {
        if self.suppress_live_output {
            let sent = self.emit_ui_event(|task_id| AgentUiEvent::ToolCall {
                task_id,
                name: name.to_string(),
                args: args.to_string(),
            });
            if sent {
                return;
            }
        }
        self.renderer.tool_call(name, args);
    }

    fn tool_result_live(&self, name: &str, args: &str, result: &str) {
        if self.suppress_live_output {
            let sent = self.emit_ui_event(|task_id| AgentUiEvent::ToolResult {
                task_id,
                name: name.to_string(),
                args: args.to_string(),
                result: result.to_string(),
            });
            if sent {
                return;
            }
        }
        self.renderer.tool_result(result);
    }

    /// Send a user message and run the full agentic loop.
    ///
    /// Returns the model's final text response. If the model invokes tools,
    /// they are executed and results are re-submitted automatically until
    /// either a text response is produced or `max_iterations` is reached.
    pub async fn send(&mut self, user_input: &str) -> Result<String, AgentError> {
        sanitize_conversation_history(&mut self.messages);
        self.messages.push(Message::user(user_input));

        if self.cancellation_requested() {
            return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
        }

        // Pre-flight context check.
        if self.tracker.is_approaching_limit(&self.messages) {
            self.warn_live("Approaching context window limit — responses may be truncated.");
        }

        let mut iterations = 0;

        loop {
            iterations += 1;
            if iterations > self.config.agent.max_iterations {
                return Err(AgentError::MaxIterationsReached);
            }

            // Build the request.
            let tool_defs = if self.tools.is_empty() {
                None
            } else {
                Some(self.tools.definitions())
            };

            let request = ChatRequest {
                model: self.config.api.model.clone(),
                messages: self.messages.clone(),
                tools: tool_defs,
                temperature: self.config.agent.temperature,
                top_p: self.config.agent.top_p,
            };

            // Call the API.
            let response = {
                let phase = if iterations == 1 {
                    format!("calling model {}", self.config.api.model)
                } else {
                    format!("calling model {} (follow-up)", self.config.api.model)
                };
                let _progress = self.renderer.progress(&phase);
                if let Some(cancel_rx) = &self.cancellation_rx {
                    let mut cancel_rx = cancel_rx.clone();
                    tokio::select! {
                        _ = wait_for_cancellation(&mut cancel_rx) => {
                            return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
                        }
                        response = self.client.chat(&request) => response?,
                    }
                } else {
                    self.client.chat(&request).await?
                }
            };

            // Record token usage if provided.
            if let Some(usage) = &response.usage {
                self.tracker
                    .record(usage.prompt_tokens, usage.completion_tokens);
                if self.config.display.show_tokens {
                    self.token_usage_live(
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        self.tracker.session_total(),
                    );
                }
            }

            // Extract the first choice.
            let choice = response
                .choices
                .into_iter()
                .next()
                .ok_or(AgentError::EmptyResponse)?;

            let mut assistant_msg = choice.message;
            sanitize_message(&mut assistant_msg);
            let has_tool_calls = assistant_msg
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty());

            // Show reasoning/thinking traces when providers emit them.
            for (field, trace) in reasoning_traces(&assistant_msg) {
                self.reasoning_trace_live(&field, &trace);
            }

            // Add meaningful assistant messages to history.
            if should_keep_message(&assistant_msg) {
                self.messages.push(assistant_msg.clone());
            }

            if has_tool_calls {
                // Execute each tool call and push results back.
                let tool_calls = assistant_msg.tool_calls.unwrap();
                let mut cancelled = false;
                for (idx, tc) in tool_calls.iter().enumerate() {
                    if self.config.display.show_tool_calls {
                        self.tool_call_live(&tc.function.name, &tc.function.arguments);
                    }

                    // `run_shell` manages its own spinner so confirmation prompts remain clean.
                    let _tool_progress = (tc.function.name != "run_shell").then(|| {
                        self.renderer
                            .progress(&format!("running tool {}", tc.function.name))
                    });

                    let result = if cancelled || self.cancellation_requested() {
                        cancelled = true;
                        CANCELLED_BY_USER_TOOL_RESULT.to_string()
                    } else if let Some(cancel_rx) = &self.cancellation_rx {
                        let mut cancel_rx = cancel_rx.clone();
                        tokio::select! {
                            _ = wait_for_cancellation(&mut cancel_rx) => {
                                cancelled = true;
                                CANCELLED_BY_USER_TOOL_RESULT.to_string()
                            }
                            exec = self.tools.execute(&tc.function.name, &tc.function.arguments) => {
                                match exec {
                                    Ok(output) => output,
                                    Err(err) => format!("Tool error: {err}"),
                                }
                            }
                        }
                    } else {
                        match self
                            .tools
                            .execute(&tc.function.name, &tc.function.arguments)
                            .await
                        {
                            Ok(output) => output,
                            Err(err) => format!("Tool error: {err}"),
                        }
                    };

                    if self.config.display.show_tool_calls {
                        self.tool_result_live(&tc.function.name, &tc.function.arguments, &result);
                    }

                    self.messages.push(Message::tool_result(&tc.id, &result));

                    if cancelled {
                        for remaining_tc in tool_calls.iter().skip(idx + 1) {
                            self.messages.push(Message::tool_result(
                                &remaining_tc.id,
                                CANCELLED_BY_USER_TOOL_RESULT,
                            ));
                        }
                        return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
                    }
                }

                // Loop back — re-submit with tool results.
                continue;
            }

            // No tool calls — this is the final text response.
            let content = assistant_msg.content.unwrap_or_default();
            return Ok(content);
        }
    }

    /// Access the conversation message history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Access the token tracker.
    pub fn tracker(&self) -> &TokenTracker {
        &self.tracker
    }
}

async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if *cancel_rx.borrow() {
        return;
    }
    let _ = cancel_rx.changed().await;
}

fn reasoning_traces(message: &Message) -> Vec<(String, String)> {
    message
        .extra
        .iter()
        .filter_map(|(key, value)| {
            if !is_reasoning_key(key) {
                return None;
            }
            reasoning_value_to_text(value).map(|text| (key.clone(), text))
        })
        .collect()
}

fn initial_messages(config: &Config) -> Vec<Message> {
    if config.agent.system_prompt.trim().is_empty() {
        Vec::new()
    } else {
        vec![Message::system(&config.agent.system_prompt)]
    }
}

fn is_reasoning_key(key: &str) -> bool {
    let k = key.to_lowercase();
    k.contains("reasoning") || k.contains("thinking") || k.contains("thought")
}

fn reasoning_value_to_text(value: &Value) -> Option<String> {
    let mut lines = Vec::<String>::new();
    collect_reasoning_strings(value, None, &mut lines);
    let mut unique = Vec::<String>::new();
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if unique.iter().any(|existing| existing == trimmed) {
            continue;
        }
        unique.push(trimmed.to_string());
    }
    if unique.is_empty() {
        None
    } else {
        Some(unique.join("\n"))
    }
}

fn collect_reasoning_strings(value: &Value, key: Option<&str>, out: &mut Vec<String>) {
    match value {
        Value::Null => {}
        Value::String(text) => {
            if key.is_none_or(is_reasoning_text_key) {
                out.push(text.clone());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_reasoning_strings(item, key, out);
            }
        }
        Value::Object(map) => {
            for (child_key, child_value) in map {
                collect_reasoning_strings(child_value, Some(child_key.as_str()), out);
            }
        }
        Value::Bool(_) | Value::Number(_) => {}
    }
}

fn is_reasoning_text_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "reasoning"
            | "reasoning_text"
            | "reasoning_content"
            | "reasoning_stream"
            | "thinking"
            | "thought"
            | "summary"
            | "summary_text"
            | "text"
            | "content"
            | "content_text"
            | "output_text"
            | "input_text"
            | "details"
            | "analysis"
            | "explanation"
    )
}

fn sanitize_conversation_history(messages: &mut Vec<Message>) {
    for message in messages.iter_mut() {
        sanitize_message(message);
    }
    messages.retain(should_keep_message);
}

fn sanitize_message(message: &mut Message) {
    if let Some(content) = message.content.as_mut() {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            message.content = None;
        } else if trimmed.len() != content.len() {
            *content = trimmed.to_string();
        }
    }

    if let Some(tool_calls) = message.tool_calls.as_mut() {
        tool_calls.retain(|tc| {
            !tc.id.trim().is_empty()
                && !tc.function.name.trim().is_empty()
                && !tc.function.arguments.trim().is_empty()
        });
        if tool_calls.is_empty() {
            message.tool_calls = None;
        }
    }

    if let Some(tool_call_id) = message.tool_call_id.as_mut() {
        let trimmed = tool_call_id.trim();
        if trimmed.is_empty() {
            message.tool_call_id = None;
        } else if trimmed.len() != tool_call_id.len() {
            *tool_call_id = trimmed.to_string();
        }
    }

    message
        .extra
        .retain(|_, value| !value.is_null() && !is_empty_json_string(value));
}

fn should_keep_message(message: &Message) -> bool {
    match message.role {
        Role::System | Role::User => message.content.is_some(),
        Role::Assistant => {
            message.content.is_some()
                || message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
        }
        Role::Tool => message.tool_call_id.is_some(),
    }
}

fn is_empty_json_string(value: &Value) -> bool {
    value
        .as_str()
        .map(|text| text.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tools::ToolRegistry;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn reasoning_key_detection() {
        assert!(is_reasoning_key("reasoning_content"));
        assert!(is_reasoning_key("thinking"));
        assert!(is_reasoning_key("chain_of_thought"));
        assert!(!is_reasoning_key("metadata"));
    }

    #[test]
    fn extracts_reasoning_traces_from_message_extra() {
        let mut msg = Message::user("hello");
        msg.extra.insert("metadata".into(), json!({"x": 1}));
        msg.extra
            .insert("reasoning_content".into(), json!("step one\nstep two"));
        msg.extra
            .insert("thinking".into(), json!({"summary": "done"}));

        let traces = reasoning_traces(&msg);
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].0, "reasoning_content");
        assert!(traces[0].1.contains("step one"));
        assert_eq!(traces[1].0, "thinking");
        assert!(traces[1].1.contains("done"));
    }

    #[test]
    fn reasoning_value_to_text_ignores_null_and_metadata_ids() {
        let value = json!([
            {
                "id": "rs_123",
                "type": "reasoning",
                "summary": []
            }
        ]);
        assert!(reasoning_value_to_text(&value).is_none());
    }

    #[test]
    fn reasoning_value_to_text_extracts_nested_text_fields() {
        let value = json!({
            "summary": [
                { "type": "summary_text", "text": "first step" },
                { "type": "summary_text", "text": "second step" }
            ],
            "id": "ignore-me"
        });
        let text = reasoning_value_to_text(&value).expect("text");
        assert!(text.contains("first step"));
        assert!(text.contains("second step"));
        assert!(!text.contains("ignore-me"));
    }

    #[test]
    fn sanitize_history_drops_empty_assistant_messages() {
        let mut messages = vec![
            Message::system("sys"),
            Message::user("u"),
            Message {
                role: Role::Assistant,
                content: None,
                tool_calls: None,
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            },
        ];
        sanitize_conversation_history(&mut messages);
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.role != Role::Assistant));
    }

    #[test]
    fn suppressed_warning_is_forwarded_to_ui_sink() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.set_live_output_suppressed(true);
        agent.set_live_output_sink(Some((9, tx)));
        agent.warn_live("hello");

        let event = rx.try_recv().expect("expected warning event");
        match event {
            AgentUiEvent::Warning { task_id, message } => {
                assert_eq!(task_id, 9);
                assert_eq!(message, "hello");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn suppressed_reasoning_is_forwarded_to_ui_sink() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.set_live_output_suppressed(true);
        agent.set_live_output_sink(Some((4, tx)));
        agent.reasoning_trace_live("reasoning_content", "step one");

        let event = rx.try_recv().expect("expected reasoning event");
        match event {
            AgentUiEvent::ReasoningTrace {
                task_id,
                field,
                trace,
            } => {
                assert_eq!(task_id, 4);
                assert_eq!(field, "reasoning_content");
                assert_eq!(trace, "step one");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn snapshot_and_restore_round_trip() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        agent.messages.push(Message::user("hello"));
        agent.tracker.record(11, 7);

        let snapshot = agent.snapshot_session();
        agent.reset_session();
        assert!(!agent.messages.iter().any(|m| {
            m.content
                .as_deref()
                .is_some_and(|content| content == "hello")
        }));

        agent.restore_session(snapshot);
        assert!(agent.messages.iter().any(|m| {
            m.content
                .as_deref()
                .is_some_and(|content| content == "hello")
        }));
        assert_eq!(agent.tracker.last_prompt_tokens, 11);
        assert_eq!(agent.tracker.last_completion_tokens, 7);
    }

    #[test]
    fn switch_api_config_updates_model_and_context_limit() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        let replacement = ApiConfig {
            base_url: "https://example.com/v1".to_string(),
            api_key: "secret".to_string(),
            model: "moonshot-v1".to_string(),
            protocol: crate::config::ApiProtocol::Completions,
            auth: crate::config::AuthMode::ApiKey,
            profile: "test".to_string(),
            context_limit: Some(42_000),
        };

        agent.switch_api_config(replacement);

        assert_eq!(agent.config.api.base_url, "https://example.com/v1");
        assert_eq!(agent.config.api.model, "moonshot-v1");
        assert_eq!(agent.tracker.context_limit, 42_000);
    }
}
