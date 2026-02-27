//! Core agentic loop.
//!
//! The [`Agent`] drives the conversation: it sends user messages to the API,
//! handles tool call responses by executing tools and re-submitting results,
//! and loops until the model produces a final text response (or the iteration
//! cap is reached).

use crate::api::{ApiClient, ModelClient};
use crate::config::{ApiConfig, Config};
use crate::error::AgentError;
use crate::render::Renderer;
use crate::runtime::{
    MetricsEvent, ModelEvent, RuntimeEvent, RuntimeEventEnvelope, TaskEvent, TaskRef, ToolEvent,
    WarningEvent,
};
use crate::tokens::{self, TokenTracker};
use crate::tools::{ToolContext, ToolRegistry, ToolStreamEvent};
use crate::types::{ChatRequest, Message, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, watch};

const CANCELLED_BY_USER_TOOL_RESULT: &str = "operation cancelled by user";
const CANCELLED_BY_USER_PROMPT_RESPONSE: &str = "operation cancelled by user";
const CONTEXT_WARNING_FRACTION: f64 = 0.80;
const CONTEXT_HARD_LIMIT_FRACTION: f64 = 0.95;
const CONTEXT_AUTO_COMPACT_TARGET_FRACTION: f64 = 0.82;
const CONTEXT_MANUAL_COMPACT_TARGET_FRACTION: f64 = 0.60;
const CONTEXT_COMPACT_KEEP_RECENT_TURNS: usize = 3;
const MAX_COMPACT_SUMMARY_LINES: usize = 24;
const COMPACT_SUMMARY_PREFIX: &str = "[buddy compact summary]";

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
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub last_prompt_tokens: u64,
    pub last_completion_tokens: u64,
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

/// Details about one history-compaction operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCompactionReport {
    pub estimated_before: u64,
    pub estimated_after: u64,
    pub removed_messages: usize,
    pub removed_turns: usize,
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
        prompt_tokens: u64,
        completion_tokens: u64,
        session_total: u64,
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

/// Stream-capable runner facade around [`Agent`].
///
/// This is a migration-friendly entry point for callers that want an explicit
/// "runner" object while the legacy `Agent::send` API remains available.
pub struct AgentRunner<'a> {
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
    client: Box<dyn ModelClient>,
    config: Config,
    tools: ToolRegistry,
    messages: Vec<Message>,
    tracker: TokenTracker,
    renderer: Renderer,
    suppress_live_output: bool,
    live_output_sink: Option<(u64, mpsc::UnboundedSender<AgentUiEvent>)>,
    runtime_event_sink: Option<(u64, mpsc::UnboundedSender<RuntimeEventEnvelope>)>,
    runtime_event_seq: u64,
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

    /// Compact older conversation turns into a synthesized summary message.
    ///
    /// This is used by `/compact` and can also be triggered automatically
    /// before request submission when context pressure is high.
    pub fn compact_history(&mut self) -> Option<HistoryCompactionReport> {
        compact_history_with_budget(
            &mut self.messages,
            self.tracker.context_limit,
            CONTEXT_MANUAL_COMPACT_TARGET_FRACTION,
            true,
        )
    }

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

    /// Route normalized runtime events to a dedicated stream consumer.
    ///
    /// This is a migration bridge while the CLI still consumes `AgentUiEvent`.
    /// Events forwarded here are adapted into `RuntimeEventEnvelope`.
    pub fn set_runtime_event_sink(
        &mut self,
        sink: Option<(u64, mpsc::UnboundedSender<RuntimeEventEnvelope>)>,
    ) {
        self.runtime_event_sink = sink;
        self.runtime_event_seq = 0;
    }

    /// Register a cancellation signal for the current in-flight request.
    pub fn set_cancellation_receiver(&mut self, rx: Option<watch::Receiver<bool>>) {
        self.cancellation_rx = rx;
    }

    fn cancellation_requested(&self) -> bool {
        self.cancellation_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    fn current_task_id(&self) -> Option<u64> {
        self.live_output_sink
            .as_ref()
            .map(|(task_id, _)| *task_id)
            .or_else(|| {
                self.runtime_event_sink
                    .as_ref()
                    .map(|(task_id, _)| *task_id)
            })
    }

    fn current_task_ref(&self) -> Option<TaskRef> {
        self.runtime_event_sink
            .as_ref()
            .map(|(task_id, _)| TaskRef::from_task_id(*task_id))
    }

    fn emit_ui_event(&mut self, event: AgentUiEvent) -> bool {
        self.live_output_sink
            .as_ref()
            .is_some_and(|(_, tx)| tx.send(event).is_ok())
    }

    fn emit_runtime_event(&mut self, event: RuntimeEvent) -> bool {
        let Some((_, tx)) = &self.runtime_event_sink else {
            return false;
        };
        let envelope = RuntimeEventEnvelope::new(self.runtime_event_seq, event);
        self.runtime_event_seq = self.runtime_event_seq.saturating_add(1);
        tx.send(envelope).is_ok()
    }

    fn warn_live(&mut self, msg: &str) {
        if let Some(task) = self.current_task_ref() {
            let _ = self.emit_runtime_event(RuntimeEvent::Warning(WarningEvent {
                task: Some(task),
                message: msg.to_string(),
            }));
        }

        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.warn(msg);
                return;
            };
            let sent = self.emit_ui_event(AgentUiEvent::Warning {
                task_id,
                message: msg.to_string(),
            });
            if sent {
                return;
            }
        }
        self.renderer.warn(msg);
    }

    fn token_usage_live(&mut self, prompt: u64, completion: u64, session_total: u64) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.token_usage(prompt, completion, session_total);
                return;
            };
            let sent = self.emit_ui_event(AgentUiEvent::TokenUsage {
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

    fn reasoning_trace_live(&mut self, field: &str, trace: &str) {
        if let Some(task) = self.current_task_ref() {
            let _ = self.emit_runtime_event(RuntimeEvent::Model(ModelEvent::ReasoningDelta {
                task,
                field: field.to_string(),
                delta: trace.to_string(),
            }));
        }

        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.reasoning_trace(field, trace);
                return;
            };
            let sent = self.emit_ui_event(AgentUiEvent::ReasoningTrace {
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

    fn tool_call_live(&mut self, name: &str, args: &str) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.tool_call(name, args);
                return;
            };
            let sent = self.emit_ui_event(AgentUiEvent::ToolCall {
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

    fn tool_result_live(&mut self, name: &str, args: &str, result: &str) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.tool_result(result);
                return;
            };
            let sent = self.emit_ui_event(AgentUiEvent::ToolResult {
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

    fn emit_tool_stream_event(&mut self, tool_name: &str, event: ToolStreamEvent) {
        let Some(task) = self.current_task_ref() else {
            return;
        };
        let runtime_event = match event {
            ToolStreamEvent::Started { detail } => RuntimeEvent::Tool(ToolEvent::CallStarted {
                task,
                name: tool_name.to_string(),
                detail,
            }),
            ToolStreamEvent::StdoutChunk { chunk } => RuntimeEvent::Tool(ToolEvent::StdoutChunk {
                task,
                name: tool_name.to_string(),
                chunk,
            }),
            ToolStreamEvent::StderrChunk { chunk } => RuntimeEvent::Tool(ToolEvent::StderrChunk {
                task,
                name: tool_name.to_string(),
                chunk,
            }),
            ToolStreamEvent::Info { message } => RuntimeEvent::Tool(ToolEvent::Info {
                task,
                name: tool_name.to_string(),
                message,
            }),
            ToolStreamEvent::Completed { detail } => RuntimeEvent::Tool(ToolEvent::Completed {
                task,
                name: tool_name.to_string(),
                detail,
            }),
        };
        let _ = self.emit_runtime_event(runtime_event);
    }

    /// Send a user message and run the full agentic loop.
    ///
    /// Returns the model's final text response. If the model invokes tools,
    /// they are executed and results are re-submitted automatically until
    /// either a text response is produced or `max_iterations` is reached.
    pub async fn send(&mut self, user_input: &str) -> Result<String, AgentError> {
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
        if let Err(err) = self.enforce_context_budget() {
            if let Some(task) = self.current_task_ref() {
                let _ = self.emit_runtime_event(RuntimeEvent::Task(TaskEvent::Failed {
                    task,
                    message: err.to_string(),
                }));
            }
            return Err(err);
        }

        let mut iterations = 0;

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
                let _progress = self.renderer.progress(&phase);
                if let Some(cancel_rx) = &self.cancellation_rx {
                    let mut cancel_rx = cancel_rx.clone();
                    tokio::select! {
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

async fn wait_for_cancellation(cancel_rx: &mut watch::Receiver<bool>) {
    if *cancel_rx.borrow() {
        return;
    }
    let _ = cancel_rx.changed().await;
}

fn compact_history_with_budget(
    messages: &mut Vec<Message>,
    context_limit: usize,
    target_fraction: f64,
    force: bool,
) -> Option<HistoryCompactionReport> {
    if context_limit == 0 || messages.is_empty() {
        return None;
    }

    let estimated_before = TokenTracker::estimate_messages(messages);
    let target_tokens = ((context_limit as f64) * target_fraction).floor().max(1.0) as usize;
    if !force && estimated_before <= target_tokens {
        return None;
    }

    let mut insertion_index = leading_system_count(messages);
    let mut previous_summary = None;
    if insertion_index > 0
        && messages
            .get(insertion_index - 1)
            .is_some_and(is_compact_summary_message)
    {
        if let Some(removed) = messages.get(insertion_index - 1).cloned() {
            previous_summary = removed.content;
        }
        messages.remove(insertion_index - 1);
        insertion_index -= 1;
    }

    let mut removed_messages = Vec::new();
    let mut removed_turns = 0usize;

    loop {
        let estimated_now = TokenTracker::estimate_messages(messages);
        let turns = collect_turn_ranges(messages, insertion_index);
        if turns.len() <= CONTEXT_COMPACT_KEEP_RECENT_TURNS {
            break;
        }

        let should_remove = if force {
            estimated_now > target_tokens || turns.len() > CONTEXT_COMPACT_KEEP_RECENT_TURNS + 1
        } else {
            estimated_now > target_tokens
        };
        if !should_remove {
            break;
        }

        let turn = turns[0];
        removed_messages.extend(messages.drain(turn.start..turn.end));
        removed_turns = removed_turns.saturating_add(1);
    }

    if removed_messages.is_empty() && previous_summary.is_none() {
        return None;
    }

    let summary = build_compact_summary(previous_summary.as_deref(), &removed_messages);
    messages.insert(insertion_index, Message::system(summary));

    let mut estimated_after = TokenTracker::estimate_messages(messages);
    if estimated_after >= estimated_before {
        messages[insertion_index] = Message::system(format!(
            "{COMPACT_SUMMARY_PREFIX}\nOlder turns were compacted."
        ));
        estimated_after = TokenTracker::estimate_messages(messages);
        if estimated_after >= estimated_before {
            messages.remove(insertion_index);
            estimated_after = TokenTracker::estimate_messages(messages);
        }
    }

    Some(HistoryCompactionReport {
        estimated_before: estimated_before as u64,
        estimated_after: estimated_after as u64,
        removed_messages: removed_messages.len(),
        removed_turns,
    })
}

#[derive(Clone, Copy)]
struct TurnRange {
    start: usize,
    end: usize,
}

fn leading_system_count(messages: &[Message]) -> usize {
    messages
        .iter()
        .take_while(|message| message.role == Role::System)
        .count()
}

fn collect_turn_ranges(messages: &[Message], start_index: usize) -> Vec<TurnRange> {
    let mut turns = Vec::new();
    let mut current_start: Option<usize> = None;

    for idx in start_index..messages.len() {
        let message = &messages[idx];
        if message.role == Role::User {
            if let Some(start) = current_start {
                turns.push(TurnRange { start, end: idx });
            }
            current_start = Some(idx);
        } else if current_start.is_none() {
            current_start = Some(idx);
        }
    }

    if let Some(start) = current_start {
        turns.push(TurnRange {
            start,
            end: messages.len(),
        });
    }

    turns
}

fn is_compact_summary_message(message: &Message) -> bool {
    message.role == Role::System
        && message
            .content
            .as_deref()
            .is_some_and(|text| text.starts_with(COMPACT_SUMMARY_PREFIX))
}

fn build_compact_summary(previous_summary: Option<&str>, removed_messages: &[Message]) -> String {
    let mut lines = Vec::new();
    lines.push(COMPACT_SUMMARY_PREFIX.to_string());
    lines.push("Older turns were compacted to preserve room for newer context.".to_string());

    if let Some(previous) = previous_summary.and_then(compact_summary_body) {
        if !previous.is_empty() {
            lines.push(format!("Previously compacted summary: {previous}"));
        }
    }

    let mut added = 0usize;
    for message in removed_messages {
        if added >= MAX_COMPACT_SUMMARY_LINES {
            break;
        }
        if let Some(line) = compact_message_line(message) {
            lines.push(line);
            added += 1;
        }
    }

    if removed_messages.len() > added {
        lines.push(format!(
            "... {} additional compacted message(s) omitted",
            removed_messages.len() - added
        ));
    }

    lines.join("\n")
}

fn compact_summary_body(summary: &str) -> Option<String> {
    let mut lines = summary.lines();
    let first = lines.next()?.trim();
    if first != COMPACT_SUMMARY_PREFIX {
        return None;
    }
    let body = lines.collect::<Vec<_>>().join(" ");
    let body = body.trim();
    if body.is_empty() {
        None
    } else {
        Some(truncate_summary_preview(body))
    }
}

fn compact_message_line(message: &Message) -> Option<String> {
    match message.role {
        Role::System => None,
        Role::User => message
            .content
            .as_deref()
            .map(|text| format!("user: {}", truncate_summary_preview(text))),
        Role::Assistant => {
            let mut parts = Vec::new();
            if let Some(content) = message.content.as_deref().map(str::trim) {
                if !content.is_empty() {
                    parts.push(format!("assistant: {}", truncate_summary_preview(content)));
                }
            }
            if let Some(tool_calls) = &message.tool_calls {
                let names = tool_calls
                    .iter()
                    .map(|call| call.function.name.as_str())
                    .collect::<Vec<_>>();
                if !names.is_empty() {
                    parts.push(format!(
                        "assistant tools: {}",
                        truncate_summary_preview(&names.join(", "))
                    ));
                }
            }
            (!parts.is_empty()).then(|| parts.join(" | "))
        }
        Role::Tool => {
            let id = message.tool_call_id.as_deref().unwrap_or("<unknown>");
            let content = message.content.as_deref().unwrap_or("");
            Some(format!(
                "tool ({id}): {}",
                truncate_summary_preview(content)
            ))
        }
    }
}

fn truncate_summary_preview(text: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 180;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX_PREVIEW_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed
        .chars()
        .take(MAX_PREVIEW_CHARS.saturating_sub(3))
        .collect::<String>();
    format!("{prefix}...")
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

    struct MockClient {
        responses: StdMutex<VecDeque<ChatResponse>>,
    }

    impl MockClient {
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
}
