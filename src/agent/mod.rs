//! Core agentic loop.
//!
//! The [`Agent`] drives the conversation: it sends user messages to the API,
//! handles tool call responses by executing tools and re-submitting results,
//! and loops until the model produces a final text response (or the iteration
//! cap is reached).

use crate::api::{ApiClient, ModelClient};
use crate::config::{ApiConfig, Config};
use crate::error::AgentError;
use crate::runtime::{
    MetricsEvent, ModelEvent, RuntimeEvent, RuntimeEventEnvelope, TaskEvent, ToolEvent,
};
use crate::tokens::{self, TokenTracker};
use crate::tools::{ToolContext, ToolRegistry};
use crate::types::{ChatRequest, Message};
use crate::ui::render::Renderer;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};

mod events;
mod history;
mod normalization;
mod prompt_aug;

pub use events::AgentUiEvent;
use history::compact_history_with_budget;
pub use history::HistoryCompactionReport;
#[cfg(test)]
use history::COMPACT_SUMMARY_PREFIX;
#[cfg(test)]
use normalization::is_reasoning_key;
#[cfg(test)]
use normalization::reasoning_value_to_text;
use normalization::{
    reasoning_traces, sanitize_conversation_history, sanitize_message, should_keep_message,
};

/// Tool-result placeholder inserted when cancellation interrupts tool execution.
const CANCELLED_BY_USER_TOOL_RESULT: &str = "operation cancelled by user";
/// Final response text returned when user cancellation wins the race.
const CANCELLED_BY_USER_PROMPT_RESPONSE: &str = "operation cancelled by user";
/// Soft threshold for emitting context-usage warnings.
const CONTEXT_WARNING_FRACTION: f64 = 0.80;
/// Hard threshold where compaction/error enforcement kicks in.
const CONTEXT_HARD_LIMIT_FRACTION: f64 = 0.95;
/// Target fraction after automatic (non-forced) compaction.
const CONTEXT_AUTO_COMPACT_TARGET_FRACTION: f64 = 0.82;
/// Target fraction for explicit/manual compaction.
const CONTEXT_MANUAL_COMPACT_TARGET_FRACTION: f64 = 0.60;

/// Persistable conversation + token state for session save/resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionSnapshot {
    /// Conversation history at snapshot time.
    pub messages: Vec<Message>,
    /// Token accounting snapshot at snapshot time.
    pub tracker: TokenTrackerSnapshot,
}

/// Persistable mirror of [`TokenTracker`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTrackerSnapshot {
    /// Active context limit used for budgeting.
    pub context_limit: usize,
    /// Cumulative prompt tokens across the session.
    pub total_prompt_tokens: u64,
    /// Cumulative completion tokens across the session.
    pub total_completion_tokens: u64,
    /// Prompt tokens for the most recent request.
    pub last_prompt_tokens: u64,
    /// Completion tokens for the most recent request.
    pub last_completion_tokens: u64,
}

impl TokenTrackerSnapshot {
    /// Capture a serializable snapshot from the live token tracker.
    fn from_tracker(tracker: &TokenTracker) -> Self {
        Self {
            context_limit: tracker.context_limit,
            total_prompt_tokens: tracker.total_prompt_tokens,
            total_completion_tokens: tracker.total_completion_tokens,
            last_prompt_tokens: tracker.last_prompt_tokens,
            last_completion_tokens: tracker.last_completion_tokens,
        }
    }

    /// Rebuild a live token tracker from serialized snapshot values.
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

/// Stream-capable runner facade around [`Agent`].
///
/// This is a migration-friendly entry point for callers that want an explicit
/// "runner" object while the legacy `Agent::send` API remains available.
pub struct AgentRunner<'a> {
    /// Mutable reference to the underlying agent instance.
    agent: &'a mut Agent,
}

impl<'a> AgentRunner<'a> {
    /// Run one prompt through the full agent loop.
    pub async fn run_prompt(&mut self, user_input: &str) -> Result<String, AgentError> {
        self.agent.send(user_input).await
    }
}

/// The core agent that orchestrates the conversation and tool-use loop.
pub struct Agent {
    /// Model client implementation (HTTP client in prod, mocks in tests).
    client: Box<dyn ModelClient>,
    /// Effective runtime/config settings.
    config: Config,
    /// Registered tool implementations available to the model.
    tools: ToolRegistry,
    /// Full conversation history sent on each request.
    messages: Vec<Message>,
    /// Token usage tracker and context budget state.
    tracker: TokenTracker,
    /// Terminal renderer used for live foreground UI.
    renderer: Renderer,
    /// If true, suppress direct renderer output and prefer sinks.
    suppress_live_output: bool,
    /// Optional legacy UI sink `(task_id, sender)` for background mode.
    live_output_sink: Option<(u64, mpsc::UnboundedSender<AgentUiEvent>)>,
    /// Optional runtime event sink `(task_id, sender)` for normalized events.
    runtime_event_sink: Option<(u64, mpsc::UnboundedSender<RuntimeEventEnvelope>)>,
    /// Monotonic sequence assigned to emitted runtime envelopes.
    runtime_event_seq: u64,
    /// Optional cancellation signal receiver for the in-flight request.
    cancellation_rx: Option<watch::Receiver<bool>>,
}

impl Agent {
    /// Create an agent from configuration with tools pre-registered.
    pub fn new(config: Config, tools: ToolRegistry) -> Self {
        let client = Box::new(ApiClient::new(
            &config.api,
            std::time::Duration::from_secs(config.network.api_timeout_secs),
        ));
        Self::with_client(config, tools, client)
    }

    /// Create an agent with an explicit model client implementation.
    ///
    /// Used for deterministic testing and alternative backends.
    pub fn with_client(config: Config, tools: ToolRegistry, client: Box<dyn ModelClient>) -> Self {
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
            runtime_event_sink: None,
            runtime_event_seq: 0,
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
        self.client = Box::new(ApiClient::new(
            &api,
            std::time::Duration::from_secs(self.config.network.api_timeout_secs),
        ));
        self.config.api = api;
        self.tracker.context_limit = context_limit;
    }

    /// Return a runner facade that can execute prompts.
    pub fn runner(&mut self) -> AgentRunner<'_> {
        AgentRunner { agent: self }
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

    /// Warn/compact/error when history nears or exceeds context limits.
    fn enforce_context_budget(&mut self) -> Result<(), AgentError> {
        let context_limit = self.tracker.context_limit;
        if context_limit == 0 {
            return Ok(());
        }

        let hard_limit_tokens = ((context_limit as f64) * CONTEXT_HARD_LIMIT_FRACTION)
            .floor()
            .max(1.0) as usize;
        let warning_tokens = ((context_limit as f64) * CONTEXT_WARNING_FRACTION)
            .floor()
            .max(1.0) as usize;

        let mut estimated_tokens = TokenTracker::estimate_messages(&self.messages);
        if estimated_tokens >= warning_tokens {
            let percent = ((estimated_tokens as f64 / context_limit as f64) * 100.0) as f32;
            self.warn_live(&format!(
                "Context usage is {percent:.1}% ({estimated_tokens}/{context_limit}). Use `/compact` or `/session new` if needed."
            ));
        }

        if estimated_tokens >= hard_limit_tokens {
            // Try automatic compaction before failing hard so long sessions can
            // continue without manual intervention.
            if let Some(report) = compact_history_with_budget(
                &mut self.messages,
                context_limit,
                CONTEXT_AUTO_COMPACT_TARGET_FRACTION,
                false,
            ) {
                estimated_tokens = report.estimated_after as usize;
                self.warn_live(&format!(
                    "Compacted history (removed {} turns / {} messages) to reduce context usage.",
                    report.removed_turns, report.removed_messages
                ));
            }
        }

        if estimated_tokens >= hard_limit_tokens {
            return Err(AgentError::ContextLimitExceeded {
                estimated_tokens: estimated_tokens as u64,
                context_limit: context_limit as u64,
            });
        }

        Ok(())
    }

    /// Register a cancellation signal for the current in-flight request.
    pub fn set_cancellation_receiver(&mut self, rx: Option<watch::Receiver<bool>>) {
        self.cancellation_rx = rx;
    }

    /// Return true when current request has been cancelled by caller.
    fn cancellation_requested(&self) -> bool {
        self.cancellation_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }
    /// Send a user message and run the full agentic loop.
    ///
    /// Returns the model's final text response. If the model invokes tools,
    /// they are executed and results are re-submitted automatically until
    /// either a text response is produced or `max_iterations` is reached.
    pub async fn send(&mut self, user_input: &str) -> Result<String, AgentError> {
        // Normalize history before appending a new turn so malformed provider
        // responses do not accumulate across requests.
        sanitize_conversation_history(&mut self.messages);
        self.messages.push(Message::user(user_input));
        if let Some(task) = self.current_task_ref() {
            let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Started { task }));
        }

        if self.cancellation_requested() {
            if let Some(task) = self.current_task_ref() {
                let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Completed { task }));
            }
            return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
        }

        let mut iterations = 0;

        // Iterative loop allows tool round-trips: assistant tool call -> tool
        // execution -> follow-up model request with tool results.
        loop {
            iterations += 1;
            if iterations > self.config.agent.max_iterations {
                if let Some(task) = self.current_task_ref() {
                    let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Failed {
                        task,
                        message: AgentError::MaxIterationsReached.to_string(),
                    }));
                }
                return Err(AgentError::MaxIterationsReached);
            }

            self.refresh_dynamic_tmux_snapshot_prompt().await;
            if let Err(err) = self.enforce_context_budget() {
                if let Some(task) = self.current_task_ref() {
                    let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Failed {
                        task,
                        message: err.to_string(),
                    }));
                }
                return Err(err);
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
            if let Some(task) = self.current_task_ref() {
                let context_limit = self.tracker.context_limit as u64;
                let estimated_tokens = TokenTracker::estimate_messages(&self.messages) as u64;
                let used_percent = if context_limit == 0 {
                    0.0
                } else {
                    ((estimated_tokens as f64 / context_limit as f64) * 100.0) as f32
                };
                let _ =
                    self.emit_runtime_event(RuntimeEvent::Metrics(MetricsEvent::ContextUsage {
                        task: task.clone(),
                        estimated_tokens,
                        context_limit,
                        used_percent,
                    }));
                let _ = self.emit_runtime_event(RuntimeEvent::Model(ModelEvent::RequestStarted {
                    task,
                    model: request.model.clone(),
                }));
            }

            // Call the API.
            let response_result = {
                let phase = if iterations == 1 {
                    format!("calling model {}", self.config.api.model)
                } else {
                    format!("calling model {} (follow-up)", self.config.api.model)
                };
                // Runtime/background execution has its own centralized liveness UI in the REPL.
                // Spawning another spinner thread here causes prompt/status overlap and flicker.
                let _progress =
                    (!self.suppress_live_output).then(|| self.renderer.progress(&phase));
                if let Some(cancel_rx) = &self.cancellation_rx {
                    let mut cancel_rx = cancel_rx.clone();
                    tokio::select! {
                        // Cancellation wins immediately and exits the entire request.
                        _ = wait_for_cancellation(&mut cancel_rx) => {
                            if let Some(task) = self.current_task_ref() {
                                let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Completed { task }));
                            }
                            return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
                        }
                        response = self.client.chat(&request) => response,
                    }
                } else {
                    self.client.chat(&request).await
                }
            };
            let response = match response_result {
                Ok(response) => response,
                Err(err) => {
                    if let Some(task) = self.current_task_ref() {
                        let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Failed {
                            task,
                            message: err.to_string(),
                        }));
                    }
                    return Err(err.into());
                }
            };

            // Record token usage if provided.
            if let Some(usage) = &response.usage {
                self.tracker
                    .record(usage.prompt_tokens, usage.completion_tokens);
                if let Some(task) = self.current_task_ref() {
                    let _ =
                        self.emit_runtime_event(RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                            task,
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                            session_total_tokens: self.tracker.session_total(),
                        }));
                }
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
                    if let Some(task) = self.current_task_ref() {
                        let _ =
                            self.emit_runtime_event(RuntimeEvent::Tool(ToolEvent::CallRequested {
                                task,
                                name: tc.function.name.clone(),
                                arguments_json: tc.function.arguments.clone(),
                            }));
                    }
                    if self.config.display.show_tool_calls {
                        self.tool_call_live(&tc.function.name, &tc.function.arguments);
                    }

                    // `run_shell` manages its own spinner so confirmation prompts remain clean.
                    let _tool_progress = (tc.function.name != "run_shell").then(|| {
                        self.renderer
                            .progress(&format!("running tool {}", tc.function.name))
                    });
                    let (tool_stream_tx, mut tool_stream_rx) = mpsc::unbounded_channel();
                    let tool_context = ToolContext::with_stream(tool_stream_tx);

                    let result = if cancelled || self.cancellation_requested() {
                        cancelled = true;
                        CANCELLED_BY_USER_TOOL_RESULT.to_string()
                    } else if let Some(cancel_rx) = &self.cancellation_rx {
                        let mut cancel_rx = cancel_rx.clone();
                        tokio::select! {
                            // If cancellation arrives while a tool is running,
                            // inject synthetic cancelled results for remaining calls.
                            _ = wait_for_cancellation(&mut cancel_rx) => {
                                cancelled = true;
                                CANCELLED_BY_USER_TOOL_RESULT.to_string()
                            }
                            exec = self.tools.execute_with_context(&tc.function.name, &tc.function.arguments, &tool_context) => {
                                match exec {
                                    Ok(output) => output,
                                    Err(err) => format!("Tool error: {err}"),
                                }
                            }
                        }
                    } else {
                        match self
                            .tools
                            .execute_with_context(
                                &tc.function.name,
                                &tc.function.arguments,
                                &tool_context,
                            )
                            .await
                        {
                            Ok(output) => output,
                            Err(err) => format!("Tool error: {err}"),
                        }
                    };
                    while let Ok(stream_event) = tool_stream_rx.try_recv() {
                        self.emit_tool_stream_event(&tc.function.name, stream_event);
                    }

                    if let Some(task) = self.current_task_ref() {
                        let _ = self.emit_runtime_event(RuntimeEvent::Tool(ToolEvent::Result {
                            task,
                            name: tc.function.name.clone(),
                            arguments_json: tc.function.arguments.clone(),
                            result: result.clone(),
                        }));
                    }
                    if self.config.display.show_tool_calls {
                        self.tool_result_live(&tc.function.name, &tc.function.arguments, &result);
                    }

                    self.messages.push(Message::tool_result(&tc.id, &result));

                    if cancelled {
                        // Ensure every declared tool call receives a result
                        // message so provider-side tool-call bookkeeping stays valid.
                        for remaining_tc in tool_calls.iter().skip(idx + 1) {
                            self.messages.push(Message::tool_result(
                                &remaining_tc.id,
                                CANCELLED_BY_USER_TOOL_RESULT,
                            ));
                        }
                        if let Some(task) = self.current_task_ref() {
                            let _ =
                                self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Completed {
                                    task,
                                }));
                        }
                        return Ok(CANCELLED_BY_USER_PROMPT_RESPONSE.to_string());
                    }
                }

                // Loop back — re-submit with tool results.
                continue;
            }

            // No tool calls — this is the final text response.
            let content = assistant_msg.content.unwrap_or_default();
            if let Some(task) = self.current_task_ref() {
                let _ = self.emit_runtime_event(RuntimeEvent::Model(ModelEvent::MessageFinal {
                    task: task.clone(),
                    content: content.clone(),
                }));
                let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Completed { task }));
            }
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

/// Wait for cancellation signal state change (or return immediately if set).
async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if *cancel_rx.borrow() {
        return;
    }
    let _ = cancel_rx.changed().await;
}

/// Build initial conversation message list from configured system prompt.
fn initial_messages(config: &Config) -> Vec<Message> {
    if config.agent.system_prompt.trim().is_empty() {
        Vec::new()
    } else {
        vec![Message::system(&config.agent.system_prompt)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModelClient;
    use crate::config::Config;
    use crate::error::{AgentError, ApiError, ToolError};
    use crate::runtime::{
        MetricsEvent, ModelEvent, RuntimeEvent, TaskEvent, ToolEvent, WarningEvent,
    };
    use crate::tools::ToolRegistry;
    use crate::types::{
        ChatRequest, ChatResponse, Choice, FunctionCall, FunctionDefinition, Message, Role,
        ToolCall, ToolDefinition, Usage,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    // Verifies reasoning key detector accepts known aliases and rejects non-reasoning keys.
    #[test]
    fn reasoning_key_detection() {
        assert!(is_reasoning_key("reasoning_content"));
        assert!(is_reasoning_key("thinking"));
        assert!(is_reasoning_key("chain_of_thought"));
        assert!(!is_reasoning_key("metadata"));
    }

    // Verifies reasoning trace extraction reads multiple reasoning-like fields.
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

    // Verifies reasoning extraction ignores null/metadata-only payloads.
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

    // Verifies nested summary/text structures are flattened into reasoning text.
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

    // Verifies sanitization drops assistant messages that have no content/tool calls.
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

    // Verifies suppressed warnings are forwarded to legacy UI event sink.
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

    // Verifies suppressed warnings are emitted on runtime event sink with task ref.
    #[test]
    fn suppressed_warning_is_forwarded_to_runtime_sink() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.set_live_output_suppressed(true);
        agent.set_runtime_event_sink(Some((12, tx)));
        agent.warn_live("runtime warning");

        let envelope = rx.try_recv().expect("expected runtime envelope");
        assert_eq!(envelope.seq, 0);
        assert_eq!(
            envelope.event,
            RuntimeEvent::Warning(WarningEvent {
                task: Some(crate::runtime::TaskRef::from_task_id(12)),
                message: "runtime warning".to_string(),
            })
        );
    }

    // Verifies reasoning traces are forwarded to legacy UI sink when suppressed.
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

    // Verifies session snapshot/restore round-trips messages and token counters.
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

    // Verifies model switch updates both API config and context limit tracker.
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

    // Verifies history compaction keeps system prefix and recent turns while shrinking history.
    #[test]
    fn compact_history_replaces_old_turns_with_summary() {
        let mut agent = Agent::new(Config::default(), ToolRegistry::new());
        agent.messages = vec![Message::system("system prompt")];
        for idx in 0..8 {
            agent
                .messages
                .push(Message::user(format!("user turn {idx}")));
            agent
                .messages
                .push(assistant_message(&format!("assistant turn {idx}")));
        }
        let recent_user = "keep this newest user turn";
        agent.messages.push(Message::user(recent_user));
        agent
            .messages
            .push(assistant_message("keep this newest assistant turn"));

        agent.tracker.context_limit = 220;
        let report = agent.compact_history().expect("history should compact");

        assert!(report.removed_turns > 0);
        assert!(report.removed_messages > 0);
        assert!(report.estimated_after < report.estimated_before);
        assert_eq!(agent.messages[0].content.as_deref(), Some("system prompt"));
        assert!(agent.messages[1]
            .content
            .as_deref()
            .is_some_and(|text| text.starts_with(COMPACT_SUMMARY_PREFIX)));
        assert!(agent.messages.iter().any(|message| {
            message
                .content
                .as_deref()
                .is_some_and(|content| content == recent_user)
        }));
    }

    // Verifies oversized single-turn prompts trigger explicit context-limit errors.
    #[tokio::test]
    async fn send_returns_context_limit_error_when_single_turn_is_too_large() {
        let mock = Box::new(MockClient::new(Vec::new()));
        let mut agent = Agent::with_client(Config::default(), ToolRegistry::new(), mock);
        agent.tracker.context_limit = 64;

        let err = agent
            .send(&"x".repeat(1_200))
            .await
            .expect_err("prompt should exceed hard context limit");
        assert!(matches!(
            err,
            AgentError::ContextLimitExceeded {
                estimated_tokens: _,
                context_limit: 64
            }
        ));
    }

    /// Build an assistant message fixture with plain text content.
    fn assistant_message(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            extra: BTreeMap::new(),
        }
    }

    /// FIFO mock model client for deterministic agent tests.
    struct MockClient {
        /// Queued responses returned in order.
        responses: StdMutex<VecDeque<ChatResponse>>,
    }

    impl MockClient {
        /// Create a mock client from a vector of canned responses.
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
            }
        }
    }

    #[async_trait]
    impl ModelClient for MockClient {
        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse, ApiError> {
            self.responses
                .lock()
                .expect("lock")
                .pop_front()
                .ok_or_else(|| ApiError::InvalidResponse("no mock response queued".to_string()))
        }
    }

    /// Model client that records incoming requests for later assertions.
    struct RecordingClient {
        /// Queued responses returned in order.
        responses: StdMutex<VecDeque<ChatResponse>>,
        /// Captured requests observed by `chat`.
        requests: StdMutex<Vec<ChatRequest>>,
    }

    impl RecordingClient {
        /// Create a recording client with canned responses.
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                requests: StdMutex::new(Vec::new()),
            }
        }

        /// Return a cloned snapshot of all captured requests.
        fn requests(&self) -> Vec<ChatRequest> {
            self.requests.lock().expect("requests lock").clone()
        }
    }

    #[async_trait]
    impl ModelClient for RecordingClient {
        async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
            self.requests
                .lock()
                .expect("requests lock")
                .push(request.clone());
            self.responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .ok_or_else(|| ApiError::InvalidResponse("no mock response queued".to_string()))
        }
    }

    #[async_trait]
    impl ModelClient for std::sync::Arc<RecordingClient> {
        async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse, ApiError> {
            (**self).chat(request).await
        }
    }

    /// Simple tool fixture that always returns a fixed success payload.
    struct EchoTool;

    #[async_trait]
    impl crate::tools::Tool for EchoTool {
        fn name(&self) -> &'static str {
            "echo_tool"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "echo_tool".to_string(),
                    description: "echo".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" }
                        }
                    }),
                },
            }
        }

        async fn execute(
            &self,
            _arguments: &str,
            _context: &crate::tools::ToolContext,
        ) -> Result<String, ToolError> {
            Ok("tool-ok".to_string())
        }
    }

    /// Tool fixture that returns queued tmux snapshot strings.
    struct SnapshotCaptureTool {
        /// Queue of snapshot strings returned in order.
        snapshots: StdMutex<VecDeque<String>>,
    }

    #[async_trait]
    impl crate::tools::Tool for SnapshotCaptureTool {
        fn name(&self) -> &'static str {
            "capture-pane"
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: "capture-pane".to_string(),
                    description: "capture pane".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {}
                    }),
                },
            }
        }

        async fn execute(
            &self,
            _arguments: &str,
            _context: &crate::tools::ToolContext,
        ) -> Result<String, ToolError> {
            let snapshot = self
                .snapshots
                .lock()
                .expect("snapshot lock")
                .pop_front()
                .unwrap_or_else(|| "snapshot-empty".to_string());
            Ok(json!({
                "harness_timestamp": {
                    "source": "harness",
                    "unix_millis": 1
                },
                "result": snapshot
            })
            .to_string())
        }
    }

    // Verifies runtime event stream ordering across a tool call round-trip.
    #[tokio::test]
    async fn runtime_stream_emits_ordered_events_for_tool_round_trip() {
        let first = ChatResponse {
            id: "r1".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "echo_tool".to_string(),
                            arguments: "{\"value\":\"x\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 5,
                completion_tokens: 2,
                total_tokens: 7,
            }),
        };
        let second = ChatResponse {
            id: "r2".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Some("final answer".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 4,
                completion_tokens: 3,
                total_tokens: 7,
            }),
        };

        let mock = Box::new(MockClient::new(vec![first, second]));
        let mut config = Config::default();
        config.display.show_tokens = false;
        config.display.show_tool_calls = false;

        let mut tools = ToolRegistry::new();
        tools.register(EchoTool);
        let mut agent = Agent::with_client(config, tools, mock);

        let (tx, mut rx) = mpsc::unbounded_channel();
        agent.set_runtime_event_sink(Some((55, tx)));

        let out = agent.send("run it").await.expect("send");
        assert_eq!(out, "final answer");

        let mut labels = Vec::new();
        while let Ok(envelope) = rx.try_recv() {
            let label = match envelope.event {
                RuntimeEvent::Task(TaskEvent::Started { .. }) => "task_started",
                RuntimeEvent::Model(ModelEvent::RequestStarted { .. }) => "model_request_started",
                RuntimeEvent::Metrics(MetricsEvent::ContextUsage { .. }) => "context_usage",
                RuntimeEvent::Metrics(MetricsEvent::TokenUsage { .. }) => "token_usage",
                RuntimeEvent::Tool(ToolEvent::CallRequested { .. }) => "tool_call",
                RuntimeEvent::Tool(ToolEvent::Result { .. }) => "tool_result",
                RuntimeEvent::Model(ModelEvent::MessageFinal { .. }) => "message_final",
                RuntimeEvent::Task(TaskEvent::Completed { .. }) => "task_completed",
                _ => "other",
            };
            labels.push(label.to_string());
        }

        let expected = vec![
            "task_started",
            "context_usage",
            "model_request_started",
            "token_usage",
            "tool_call",
            "tool_result",
            "context_usage",
            "model_request_started",
            "token_usage",
            "message_final",
            "task_completed",
        ];
        assert_eq!(labels, expected);
    }

    // Verifies dynamic tmux snapshot prompt is replaced each request, not appended.
    #[tokio::test]
    async fn tmux_snapshot_prompt_is_replaced_each_request() {
        let first = ChatResponse {
            id: "r1".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: Some(vec![ToolCall {
                        id: "call_1".to_string(),
                        call_type: "function".to_string(),
                        function: FunctionCall {
                            name: "echo_tool".to_string(),
                            arguments: "{\"value\":\"x\"}".to_string(),
                        },
                    }]),
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: None,
        };
        let second = ChatResponse {
            id: "r2".to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Some("done".to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: None,
        };

        let client = std::sync::Arc::new(RecordingClient::new(vec![first, second]));
        let mut config = Config::default();
        config.agent.system_prompt = "base system prompt".to_string();
        config.display.show_tool_calls = false;
        config.display.show_tokens = false;

        let mut tools = ToolRegistry::new();
        tools.register(SnapshotCaptureTool {
            snapshots: StdMutex::new(VecDeque::from(vec![
                "snapshot-one".to_string(),
                "snapshot-two".to_string(),
            ])),
        });
        tools.register(EchoTool);

        let mut agent = Agent::with_client(config, tools, Box::new(client.clone()));
        let out = agent.send("run").await.expect("send succeeds");
        assert_eq!(out, "done");

        let requests = client.requests();
        assert_eq!(requests.len(), 2);
        let first_system = requests[0].messages[0]
            .content
            .as_deref()
            .unwrap_or_default();
        let second_system = requests[1].messages[0]
            .content
            .as_deref()
            .unwrap_or_default();

        assert!(first_system.contains("snapshot-one"));
        assert!(!first_system.contains("snapshot-two"));
        assert!(second_system.contains("snapshot-two"));
        assert!(!second_system.contains("snapshot-one"));
    }
}
