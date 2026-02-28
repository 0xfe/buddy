//! Runtime command/event schema.
//!
//! This module contains the public control-plane and data-plane contracts used
//! by frontends to drive the runtime actor and render progress streams.

use crate::agent::AgentUiEvent;
use crate::config::{ApiProtocol, AuthMode};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Logical reference for work attached to a specific task.
///
/// `session_id` and `iteration` are optional because some current event sources
/// only know `task_id`. As runtime wiring improves, those fields can be filled
/// consistently without changing the event shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TaskRef {
    /// Runtime task id assigned by the actor.
    pub task_id: u64,
    /// Optional persisted session id related to this task.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Optional iteration index within a multi-step task flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u32>,
}

impl TaskRef {
    /// Build a task ref with only the required task id.
    pub fn from_task_id(task_id: u64) -> Self {
        Self {
            task_id,
            session_id: None,
            iteration: None,
        }
    }
}

/// Control-plane commands for a runtime actor.
///
/// These are frontend-originated requests (submit prompt, approve command,
/// cancel running task, switch session/model, shutdown).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RuntimeCommand {
    /// Queue a new prompt task for execution.
    SubmitPrompt {
        /// User prompt text.
        prompt: String,
        /// Optional metadata propagated by the caller.
        metadata: PromptMetadata,
    },
    /// Resolve a pending approval request.
    Approve {
        /// Runtime-generated approval id from `TaskEvent::WaitingApproval`.
        approval_id: String,
        /// Decision chosen by the user/policy.
        decision: ApprovalDecision,
    },
    /// Request cancellation of an active task id.
    CancelTask {
        /// Task identifier to cancel.
        task_id: u64,
    },
    /// Update runtime approval behavior.
    SetApprovalPolicy {
        /// New policy to apply.
        policy: RuntimeApprovalPolicy,
    },
    /// Switch the active model profile.
    SwitchModel {
        /// Profile name defined in config.
        profile: String,
    },
    /// Start a fresh session.
    SessionNew,
    /// Resume a specific saved session id.
    SessionResume {
        /// Session id to resume.
        session_id: String,
    },
    /// Resume the store's most recently active session.
    SessionResumeLast,
    /// Compact current session history.
    SessionCompact,
    /// Stop the runtime actor.
    Shutdown,
}

/// Optional metadata attached to a submitted prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptMetadata {
    /// Origin tag for analytics/debugging (for example `cli`, `api`, `test`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Optional request correlation id for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// Decision for a pending approval request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    /// Approve and continue execution.
    Approve,
    /// Deny and block execution.
    Deny,
}

/// Runtime-level approval policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "mode")]
pub enum RuntimeApprovalPolicy {
    /// Always ask for explicit user approval.
    Ask,
    /// Auto-approve all requests.
    All,
    /// Auto-deny all requests.
    None,
    /// Auto-approve until the unix timestamp expires.
    Until {
        /// Absolute unix timestamp in milliseconds when auto-approve expires.
        expires_at_unix_ms: u64,
    },
}

/// Monotonic envelope for runtime events.
///
/// `seq` is assigned by the runtime/event source; `ts_unix_ms` is wall-clock
/// capture time used for diagnostics, tracing, and playback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeEventEnvelope {
    /// Monotonic sequence assigned by runtime event source.
    pub seq: u64,
    /// Wall-clock timestamp (unix milliseconds) at envelope creation.
    pub ts_unix_ms: u64,
    /// Typed runtime event payload.
    pub event: RuntimeEvent,
}

impl RuntimeEventEnvelope {
    /// Build a new envelope around an event.
    pub fn new(seq: u64, event: RuntimeEvent) -> Self {
        Self {
            seq,
            ts_unix_ms: now_unix_millis(),
            event,
        }
    }
}

/// Typed runtime event families.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "payload")]
pub enum RuntimeEvent {
    /// Runtime startup/shutdown/config milestones.
    Lifecycle(LifecycleEvent),
    /// Session lifecycle updates.
    Session(SessionEvent),
    /// Task queue/lifecycle updates.
    Task(TaskEvent),
    /// Model request/response stream updates.
    Model(ModelEvent),
    /// Tool call execution updates.
    Tool(ToolEvent),
    /// Numeric usage and duration metrics.
    Metrics(MetricsEvent),
    /// Non-fatal warnings.
    Warning(WarningEvent),
    /// Recoverable or fatal command/task errors.
    Error(ErrorEvent),
}

/// Runtime lifecycle milestones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleEvent {
    /// Runtime actor started successfully.
    RuntimeStarted,
    /// Runtime actor is shutting down.
    RuntimeStopped,
    /// Runtime finished initial configuration/bootstrap.
    ConfigLoaded,
}

/// Session-scoped events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionEvent {
    /// A new session id was created and activated.
    Created { session_id: String },
    /// An existing session id was resumed and activated.
    Resumed { session_id: String },
    /// Active session snapshot was persisted.
    Saved { session_id: String },
    /// Active session history was compacted.
    Compacted { session_id: String },
}

/// Task state transitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskEvent {
    /// Task entered queue before execution.
    Queued {
        /// Logical task reference.
        task: TaskRef,
        /// Task kind (for example `prompt`).
        kind: String,
        /// Short user-facing details string.
        details: String,
    },
    /// Task started execution.
    Started {
        /// Logical task reference.
        task: TaskRef,
    },
    /// Task is paused awaiting approval.
    WaitingApproval {
        /// Logical task reference.
        task: TaskRef,
        /// Identifier required for approve/deny command.
        approval_id: String,
        /// Truncated command preview awaiting approval.
        command: String,
        /// Optional risk classification.
        #[serde(skip_serializing_if = "Option::is_none")]
        risk: Option<String>,
        /// Optional mutation hint.
        #[serde(skip_serializing_if = "Option::is_none")]
        mutation: Option<bool>,
        /// Optional privilege-escalation hint.
        #[serde(skip_serializing_if = "Option::is_none")]
        privesc: Option<bool>,
        /// Optional rationale explaining why approval is needed.
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// Cancellation was requested for this task.
    Cancelling {
        /// Logical task reference.
        task: TaskRef,
    },
    /// Task finished successfully.
    Completed {
        /// Logical task reference.
        task: TaskRef,
    },
    /// Task failed with an error message.
    Failed {
        /// Logical task reference.
        task: TaskRef,
        /// User-facing failure text.
        message: String,
    },
}

/// Model-side incremental/final output events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelEvent {
    /// Runtime switched to a different model profile.
    ProfileSwitched {
        /// Selected profile name.
        profile: String,
        /// Effective model identifier.
        model: String,
        /// Effective base URL for API calls.
        base_url: String,
        /// Effective protocol mode.
        api: ApiProtocol,
        /// Effective auth mode.
        auth: AuthMode,
    },
    /// One model request started for a task.
    RequestStarted {
        /// Logical task reference.
        task: TaskRef,
        /// Model identifier for this request.
        model: String,
    },
    /// Incremental assistant text delta.
    TextDelta {
        /// Logical task reference.
        task: TaskRef,
        /// Delta text chunk.
        delta: String,
    },
    /// Incremental reasoning/thinking text delta.
    ReasoningDelta {
        /// Logical task reference.
        task: TaskRef,
        /// Source field/key for this reasoning delta.
        field: String,
        /// Delta text chunk.
        delta: String,
    },
    /// Final assistant text message for a task.
    MessageFinal {
        /// Logical task reference.
        task: TaskRef,
        /// Final assistant message content.
        content: String,
    },
}

/// Tool invocation/result events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvent {
    /// Model requested a tool call.
    CallRequested {
        /// Logical task reference.
        task: TaskRef,
        /// Requested tool name.
        name: String,
        /// Raw tool arguments JSON from the model.
        arguments_json: String,
    },
    /// Tool execution started.
    CallStarted {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Human-friendly start detail.
        detail: String,
    },
    /// Incremental stdout chunk from tool execution.
    StdoutChunk {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Captured stdout text chunk.
        chunk: String,
    },
    /// Incremental stderr chunk from tool execution.
    StderrChunk {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Captured stderr text chunk.
        chunk: String,
    },
    /// Informational tool event message.
    Info {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Informational text.
        message: String,
    },
    /// Tool execution completed.
    Completed {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Human-friendly completion detail.
        detail: String,
    },
    /// Tool result payload attached to model round-trip history.
    Result {
        /// Logical task reference.
        task: TaskRef,
        /// Tool name.
        name: String,
        /// Raw tool arguments JSON used for execution.
        arguments_json: String,
        /// Raw tool result payload.
        result: String,
    },
}

/// Runtime metrics updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MetricsEvent {
    /// Prompt/completion token usage update.
    TokenUsage {
        /// Logical task reference.
        task: TaskRef,
        /// Prompt tokens used in the latest request.
        prompt_tokens: u64,
        /// Completion tokens used in the latest request.
        completion_tokens: u64,
        /// Running session token total.
        session_total_tokens: u64,
    },
    /// Context-window usage estimate update.
    ContextUsage {
        /// Logical task reference.
        task: TaskRef,
        /// Estimated token count for current message history.
        estimated_tokens: u64,
        /// Context window limit for active model.
        context_limit: u64,
        /// Estimated usage percent (`estimated/context_limit * 100`).
        used_percent: f32,
    },
    /// Duration metric for an internal phase.
    PhaseDuration {
        /// Logical task reference.
        task: TaskRef,
        /// Phase label.
        phase: String,
        /// Elapsed duration in milliseconds.
        elapsed_ms: u64,
    },
}

/// Non-fatal warning surfaced to frontends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WarningEvent {
    /// Optional task context for task-scoped warnings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskRef>,
    /// Warning message text.
    pub message: String,
}

/// Error surfaced to frontends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEvent {
    /// Optional task context for task-scoped errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskRef>,
    /// Error message text.
    pub message: String,
}

/// Adapter helper for migrating from the existing background UI side-channel.
pub fn runtime_event_from_agent_ui(event: AgentUiEvent) -> RuntimeEvent {
    match event {
        AgentUiEvent::Warning { task_id, message } => RuntimeEvent::Warning(WarningEvent {
            task: Some(TaskRef::from_task_id(task_id)),
            message,
        }),
        AgentUiEvent::TokenUsage {
            task_id,
            prompt_tokens,
            completion_tokens,
            session_total,
        } => RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
            task: TaskRef::from_task_id(task_id),
            prompt_tokens,
            completion_tokens,
            session_total_tokens: session_total,
        }),
        AgentUiEvent::ReasoningTrace {
            task_id,
            field,
            trace,
        } => RuntimeEvent::Model(ModelEvent::ReasoningDelta {
            task: TaskRef::from_task_id(task_id),
            field,
            delta: trace,
        }),
        AgentUiEvent::ToolCall {
            task_id,
            name,
            args,
        } => RuntimeEvent::Tool(ToolEvent::CallRequested {
            task: TaskRef::from_task_id(task_id),
            name,
            arguments_json: args,
        }),
        AgentUiEvent::ToolResult {
            task_id,
            name,
            args,
            result,
        } => RuntimeEvent::Tool(ToolEvent::Result {
            task: TaskRef::from_task_id(task_id),
            name,
            arguments_json: args,
            result,
        }),
    }
}

/// Convert an existing `AgentUiEvent` into a timestamped runtime envelope.
pub fn runtime_envelope_from_agent_ui(seq: u64, event: AgentUiEvent) -> RuntimeEventEnvelope {
    RuntimeEventEnvelope::new(seq, runtime_event_from_agent_ui(event))
}

/// Return current wall-clock unix timestamp in milliseconds.
fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
