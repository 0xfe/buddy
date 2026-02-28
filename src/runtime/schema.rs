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
    pub task_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
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
    SubmitPrompt {
        prompt: String,
        metadata: PromptMetadata,
    },
    Approve {
        approval_id: String,
        decision: ApprovalDecision,
    },
    CancelTask {
        task_id: u64,
    },
    SetApprovalPolicy {
        policy: RuntimeApprovalPolicy,
    },
    SwitchModel {
        profile: String,
    },
    SessionNew,
    SessionResume {
        session_id: String,
    },
    SessionResumeLast,
    SessionCompact,
    Shutdown,
}

/// Optional metadata attached to a submitted prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromptMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

/// Decision for a pending approval request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Runtime-level approval policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", tag = "mode")]
pub enum RuntimeApprovalPolicy {
    Ask,
    All,
    None,
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
    pub seq: u64,
    pub ts_unix_ms: u64,
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
    Lifecycle(LifecycleEvent),
    Session(SessionEvent),
    Task(TaskEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
    Metrics(MetricsEvent),
    Warning(WarningEvent),
    Error(ErrorEvent),
}

/// Runtime lifecycle milestones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleEvent {
    RuntimeStarted,
    RuntimeStopped,
    ConfigLoaded,
}

/// Session-scoped events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionEvent {
    Created { session_id: String },
    Resumed { session_id: String },
    Saved { session_id: String },
    Compacted { session_id: String },
}

/// Task state transitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskEvent {
    Queued {
        task: TaskRef,
        kind: String,
        details: String,
    },
    Started {
        task: TaskRef,
    },
    WaitingApproval {
        task: TaskRef,
        approval_id: String,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        risk: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        mutation: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        privesc: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    Cancelling {
        task: TaskRef,
    },
    Completed {
        task: TaskRef,
    },
    Failed {
        task: TaskRef,
        message: String,
    },
}

/// Model-side incremental/final output events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelEvent {
    ProfileSwitched {
        profile: String,
        model: String,
        base_url: String,
        api: ApiProtocol,
        auth: AuthMode,
    },
    RequestStarted {
        task: TaskRef,
        model: String,
    },
    TextDelta {
        task: TaskRef,
        delta: String,
    },
    ReasoningDelta {
        task: TaskRef,
        field: String,
        delta: String,
    },
    MessageFinal {
        task: TaskRef,
        content: String,
    },
}

/// Tool invocation/result events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolEvent {
    CallRequested {
        task: TaskRef,
        name: String,
        arguments_json: String,
    },
    CallStarted {
        task: TaskRef,
        name: String,
        detail: String,
    },
    StdoutChunk {
        task: TaskRef,
        name: String,
        chunk: String,
    },
    StderrChunk {
        task: TaskRef,
        name: String,
        chunk: String,
    },
    Info {
        task: TaskRef,
        name: String,
        message: String,
    },
    Completed {
        task: TaskRef,
        name: String,
        detail: String,
    },
    Result {
        task: TaskRef,
        name: String,
        arguments_json: String,
        result: String,
    },
}

/// Runtime metrics updates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MetricsEvent {
    TokenUsage {
        task: TaskRef,
        prompt_tokens: u64,
        completion_tokens: u64,
        session_total_tokens: u64,
    },
    ContextUsage {
        task: TaskRef,
        estimated_tokens: u64,
        context_limit: u64,
        used_percent: f32,
    },
    PhaseDuration {
        task: TaskRef,
        phase: String,
        elapsed_ms: u64,
    },
}

/// Non-fatal warning surfaced to frontends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WarningEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskRef>,
    pub message: String,
}

/// Error surfaced to frontends.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskRef>,
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

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
