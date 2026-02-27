//! Streaming runtime command/event schema.
//!
//! This module defines the public control-plane (`RuntimeCommand`) and data-plane
//! (`RuntimeEvent`) contracts used by stream-capable frontends. The goal is to
//! let alternate CLIs (or GUIs) drive Buddy and render progress without relying
//! on terminal-coupled internals.

use crate::agent::Agent;
use crate::agent::AgentUiEvent;
use crate::config::{select_model_profile, ApiProtocol, AuthMode, Config};
use crate::preflight::validate_active_profile_ready;
use crate::session::SessionStore;
use crate::textutil::truncate_with_suffix_by_chars;
use crate::tools::shell::ShellApprovalRequest;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch, Mutex};

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

/// Handle for sending commands to a spawned runtime actor.
#[derive(Clone)]
pub struct BuddyRuntimeHandle {
    pub commands: mpsc::Sender<RuntimeCommand>,
}

impl BuddyRuntimeHandle {
    /// Send one command to the runtime actor.
    pub async fn send(&self, command: RuntimeCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .await
            .map_err(|_| "runtime command channel closed".to_string())
    }
}

/// Event stream receiver returned by [`spawn_runtime`].
pub type RuntimeEventStream = mpsc::UnboundedReceiver<RuntimeEventEnvelope>;

/// Bootstrap inputs for the runtime actor.
pub struct RuntimeSpawnConfig {
    pub config: Config,
    pub tools: crate::tools::ToolRegistry,
    pub session_store: Option<SessionStore>,
    pub active_session: Option<String>,
    pub approval_rx: Option<mpsc::UnboundedReceiver<ShellApprovalRequest>>,
}

/// Spawn a runtime actor from config + tool registry.
pub fn spawn_runtime(config: RuntimeSpawnConfig) -> (BuddyRuntimeHandle, RuntimeEventStream) {
    let agent = Agent::new(config.config.clone(), config.tools);
    spawn_runtime_with_agent(
        agent,
        config.config,
        config.session_store,
        config.active_session,
        config.approval_rx,
    )
}

/// Spawn a runtime actor from an existing agent instance.
///
/// This entry point is primarily for tests and advanced embedding where the
/// caller wants to inject a preconfigured/mocked agent.
pub fn spawn_runtime_with_agent(
    agent: Agent,
    config: Config,
    session_store: Option<SessionStore>,
    active_session: Option<String>,
    approval_rx: Option<mpsc::UnboundedReceiver<ShellApprovalRequest>>,
) -> (BuddyRuntimeHandle, RuntimeEventStream) {
    let shared_agent = Arc::new(Mutex::new(agent));
    spawn_runtime_with_shared_agent(
        shared_agent,
        config,
        session_store,
        active_session,
        approval_rx,
    )
}

/// Spawn a runtime actor from a shared agent handle.
///
/// This lets the caller keep read-only snapshot access for status rendering
/// while the runtime owns execution orchestration.
pub fn spawn_runtime_with_shared_agent(
    agent: Arc<Mutex<Agent>>,
    config: Config,
    session_store: Option<SessionStore>,
    active_session: Option<String>,
    approval_rx: Option<mpsc::UnboundedReceiver<ShellApprovalRequest>>,
) -> (BuddyRuntimeHandle, RuntimeEventStream) {
    let (command_tx, mut command_rx) = mpsc::channel::<RuntimeCommand>(64);
    let (event_tx, event_rx) = mpsc::unbounded_channel::<RuntimeEventEnvelope>();

    tokio::spawn(async move {
        let (agent_event_tx, mut agent_event_rx) =
            mpsc::unbounded_channel::<RuntimeEventEnvelope>();
        let (task_done_tx, mut task_done_rx) = mpsc::unbounded_channel::<TaskDone>();
        let mut approval_rx = approval_rx;

        let mut seq: u64 = 0;
        let mut next_task_id: u64 = 1;
        let mut active_task: Option<ActiveTask> = None;
        let mut pending_approvals = HashMap::<String, PendingRuntimeApproval>::new();
        let mut next_approval_nonce: u64 = 1;
        let mut state = RuntimeActorState {
            config,
            session_store,
            active_session,
            approval_policy: RuntimeApprovalPolicy::Ask,
        };

        emit_event(
            &event_tx,
            &mut seq,
            RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStarted),
        );
        emit_event(
            &event_tx,
            &mut seq,
            RuntimeEvent::Lifecycle(LifecycleEvent::ConfigLoaded),
        );

        loop {
            tokio::select! {
                Some(command) = command_rx.recv() => {
                    let should_stop = handle_runtime_command(
                        command,
                        &agent,
                        &mut state,
                        &mut active_task,
                        &mut next_task_id,
                        &mut pending_approvals,
                        &event_tx,
                        &mut seq,
                        &agent_event_tx,
                        &task_done_tx,
                    ).await;
                    if should_stop {
                        emit_event(
                            &event_tx,
                            &mut seq,
                            RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStopped),
                        );
                        break;
                    }
                }
                Some(agent_envelope) = agent_event_rx.recv() => {
                    emit_event(&event_tx, &mut seq, agent_envelope.event);
                }
                Some(done) = task_done_rx.recv() => {
                    if active_task.as_ref().is_some_and(|active| active.task_id == done.task_id) {
                        active_task = None;
                    }
                    deny_pending_approvals_for_task(done.task_id, &mut pending_approvals);

                    persist_active_session_snapshot(&agent, &state, &event_tx, &mut seq).await;

                    if let Err(err) = done.result {
                        emit_event(
                            &event_tx,
                            &mut seq,
                            RuntimeEvent::Task(TaskEvent::Failed {
                                task: TaskRef::from_task_id(done.task_id),
                                message: err.to_string(),
                            }),
                        );
                    }
                }
                Some(request) = async {
                    match approval_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => None,
                    }
                } => {
                    handle_approval_request(
                        request,
                        &mut state,
                        active_task.as_ref(),
                        &mut pending_approvals,
                        &mut next_approval_nonce,
                        &event_tx,
                        &mut seq,
                    );
                }
                else => break,
            }
        }
    });

    (
        BuddyRuntimeHandle {
            commands: command_tx,
        },
        event_rx,
    )
}

struct RuntimeActorState {
    config: Config,
    session_store: Option<SessionStore>,
    active_session: Option<String>,
    approval_policy: RuntimeApprovalPolicy,
}

struct ActiveTask {
    task_id: u64,
    cancel_tx: watch::Sender<bool>,
}

struct TaskDone {
    task_id: u64,
    result: Result<String, crate::error::AgentError>,
}

struct PendingRuntimeApproval {
    task_id: u64,
    request: ShellApprovalRequest,
}

fn emit_event(
    tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
    event: RuntimeEvent,
) {
    let _ = tx.send(RuntimeEventEnvelope::new(*seq, event));
    *seq = seq.saturating_add(1);
}

async fn handle_runtime_command(
    command: RuntimeCommand,
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    active_task: &mut Option<ActiveTask>,
    next_task_id: &mut u64,
    pending_approvals: &mut HashMap<String, PendingRuntimeApproval>,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
    agent_event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    task_done_tx: &mpsc::UnboundedSender<TaskDone>,
) -> bool {
    match command {
        RuntimeCommand::SubmitPrompt { prompt, .. } => {
            if active_task.is_some() {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: "a prompt task is already running".to_string(),
                    }),
                );
                return false;
            }

            let task_id = *next_task_id;
            *next_task_id = next_task_id.saturating_add(1);
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Task(TaskEvent::Queued {
                    task: TaskRef::from_task_id(task_id),
                    kind: "prompt".to_string(),
                    details: truncate_preview(&prompt, 80),
                }),
            );

            let (cancel_tx, cancel_rx) = watch::channel(false);
            *active_task = Some(ActiveTask { task_id, cancel_tx });
            spawn_prompt_task(
                Arc::clone(agent),
                task_id,
                prompt,
                cancel_rx,
                agent_event_tx.clone(),
                task_done_tx.clone(),
            );
        }
        RuntimeCommand::CancelTask { task_id } => {
            let Some(active) = active_task.as_ref() else {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: Some(TaskRef::from_task_id(task_id)),
                        message: format!("no running task with id #{task_id}"),
                    }),
                );
                return false;
            };
            if active.task_id != task_id {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: Some(TaskRef::from_task_id(task_id)),
                        message: format!("task #{task_id} is not active"),
                    }),
                );
                return false;
            }
            deny_pending_approvals_for_task(task_id, pending_approvals);
            let _ = active.cancel_tx.send(true);
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Task(TaskEvent::Cancelling {
                    task: TaskRef::from_task_id(task_id),
                }),
            );
        }
        RuntimeCommand::SetApprovalPolicy { policy } => {
            state.approval_policy = policy;
            if let Some(decision) = active_approval_decision(&mut state.approval_policy) {
                for pending in pending_approvals.drain().map(|(_, pending)| pending) {
                    resolve_pending_approval(pending, decision, event_tx, seq);
                }
            }
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Warning(WarningEvent {
                    task: None,
                    message: "approval policy updated".to_string(),
                }),
            );
        }
        RuntimeCommand::SwitchModel { profile } => {
            if active_task.is_some() {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: "cannot switch model while a task is running".to_string(),
                    }),
                );
                return false;
            }

            let previous_protocol = state.config.api.protocol;
            let previous_auth = state.config.api.auth;
            let mut next = state.config.clone();
            if let Err(err) = select_model_profile(&mut next, &profile) {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: format!("failed to select model profile `{profile}`: {err}"),
                    }),
                );
                return false;
            }
            if let Err(err) = validate_active_profile_ready(&next) {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: err,
                    }),
                );
                return false;
            }

            state.config = next.clone();
            {
                let mut guard = agent.lock().await;
                guard.switch_api_config(next.api.clone());
            }

            if previous_protocol != next.api.protocol || previous_auth != next.api.auth {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Warning(WarningEvent {
                        task: None,
                        message: format!(
                            "model switch changed API mode: {} -> {}. Existing history is preserved; if behavior looks inconsistent, run `/session new`.",
                            api_mode_label(previous_protocol, previous_auth),
                            api_mode_label(next.api.protocol, next.api.auth)
                        ),
                    }),
                );
            }

            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Model(ModelEvent::ProfileSwitched {
                    profile: next.api.profile.clone(),
                    model: next.api.model.clone(),
                    base_url: next.api.base_url.clone(),
                    api: next.api.protocol,
                    auth: next.api.auth,
                }),
            );
        }
        RuntimeCommand::SessionNew => {
            if let Err(err) = runtime_session_new(agent, state, event_tx, seq).await {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: err,
                    }),
                );
            }
        }
        RuntimeCommand::SessionResume { session_id } => {
            if let Err(err) = runtime_session_resume(agent, state, &session_id, event_tx, seq).await
            {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: err,
                    }),
                );
            }
        }
        RuntimeCommand::SessionResumeLast => {
            let Some(store) = state.session_store.as_ref() else {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: "session store is unavailable".to_string(),
                    }),
                );
                return false;
            };
            match store.resolve_last() {
                Ok(Some(last)) => {
                    if let Err(err) =
                        runtime_session_resume(agent, state, &last, event_tx, seq).await
                    {
                        emit_event(
                            event_tx,
                            seq,
                            RuntimeEvent::Error(ErrorEvent {
                                task: None,
                                message: err,
                            }),
                        );
                    }
                }
                Ok(None) => emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: "no saved sessions found".to_string(),
                    }),
                ),
                Err(err) => emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: format!("failed to resolve last session: {err}"),
                    }),
                ),
            }
        }
        RuntimeCommand::SessionCompact => {
            if let Err(err) = runtime_session_compact(agent, state, event_tx, seq).await {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: err,
                    }),
                );
            }
        }
        RuntimeCommand::Approve {
            approval_id,
            decision,
        } => {
            let Some(pending) = pending_approvals.remove(&approval_id) else {
                emit_event(
                    event_tx,
                    seq,
                    RuntimeEvent::Error(ErrorEvent {
                        task: None,
                        message: format!("unknown approval id `{approval_id}`"),
                    }),
                );
                return false;
            };
            resolve_pending_approval(pending, decision, event_tx, seq);
        }
        RuntimeCommand::Shutdown => {
            for pending in pending_approvals.drain().map(|(_, pending)| pending) {
                pending.request.deny();
            }
            if let Some(active) = active_task.as_ref() {
                let _ = active.cancel_tx.send(true);
            }
            return true;
        }
    }
    false
}

fn handle_approval_request(
    request: ShellApprovalRequest,
    state: &mut RuntimeActorState,
    active_task: Option<&ActiveTask>,
    pending_approvals: &mut HashMap<String, PendingRuntimeApproval>,
    next_approval_nonce: &mut u64,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let Some(active_task) = active_task else {
        request.deny();
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Warning(WarningEvent {
                task: None,
                message: "approval request arrived without an active task; denied".to_string(),
            }),
        );
        return;
    };

    if let Some(decision) = active_approval_decision(&mut state.approval_policy) {
        resolve_pending_approval(
            PendingRuntimeApproval {
                task_id: active_task.task_id,
                request,
            },
            decision,
            event_tx,
            seq,
        );
        return;
    }

    let approval_id = format!("appr-{}-{:04x}", active_task.task_id, *next_approval_nonce);
    *next_approval_nonce = next_approval_nonce.saturating_add(1);
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Task(TaskEvent::WaitingApproval {
            task: TaskRef::from_task_id(active_task.task_id),
            approval_id: approval_id.clone(),
            command: truncate_preview(request.command(), 140),
        }),
    );
    pending_approvals.insert(
        approval_id,
        PendingRuntimeApproval {
            task_id: active_task.task_id,
            request,
        },
    );
}

fn active_approval_decision(policy: &mut RuntimeApprovalPolicy) -> Option<ApprovalDecision> {
    match policy {
        RuntimeApprovalPolicy::Ask => None,
        RuntimeApprovalPolicy::All => Some(ApprovalDecision::Approve),
        RuntimeApprovalPolicy::None => Some(ApprovalDecision::Deny),
        RuntimeApprovalPolicy::Until { expires_at_unix_ms } => {
            if now_unix_millis() < *expires_at_unix_ms {
                Some(ApprovalDecision::Approve)
            } else {
                *policy = RuntimeApprovalPolicy::Ask;
                None
            }
        }
    }
}

fn resolve_pending_approval(
    pending: PendingRuntimeApproval,
    decision: ApprovalDecision,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let task = TaskRef::from_task_id(pending.task_id);
    match decision {
        ApprovalDecision::Approve => {
            pending.request.approve();
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Warning(WarningEvent {
                    task: Some(task.clone()),
                    message: "approval granted".to_string(),
                }),
            );
        }
        ApprovalDecision::Deny => {
            pending.request.deny();
            emit_event(
                event_tx,
                seq,
                RuntimeEvent::Warning(WarningEvent {
                    task: Some(task.clone()),
                    message: "approval denied".to_string(),
                }),
            );
        }
    }
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Task(TaskEvent::Started { task }),
    );
}

fn deny_pending_approvals_for_task(
    task_id: u64,
    pending_approvals: &mut HashMap<String, PendingRuntimeApproval>,
) {
    let approval_ids = pending_approvals
        .iter()
        .filter_map(|(id, pending)| (pending.task_id == task_id).then_some(id.clone()))
        .collect::<Vec<_>>();
    for approval_id in approval_ids {
        if let Some(pending) = pending_approvals.remove(&approval_id) {
            pending.request.deny();
        }
    }
}

fn spawn_prompt_task(
    agent: Arc<Mutex<Agent>>,
    task_id: u64,
    prompt: String,
    cancel_rx: watch::Receiver<bool>,
    event_tx: mpsc::UnboundedSender<RuntimeEventEnvelope>,
    done_tx: mpsc::UnboundedSender<TaskDone>,
) {
    tokio::spawn(async move {
        let mut agent = agent.lock().await;
        agent.set_live_output_suppressed(true);
        agent.set_live_output_sink(None);
        agent.set_runtime_event_sink(Some((task_id, event_tx)));
        agent.set_cancellation_receiver(Some(cancel_rx));
        let result = agent.send(&prompt).await;
        agent.set_cancellation_receiver(None);
        agent.set_runtime_event_sink(None);
        agent.set_live_output_suppressed(false);
        drop(agent);
        let _ = done_tx.send(TaskDone { task_id, result });
    });
}

async fn runtime_session_new(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    let snapshot = {
        let mut guard = agent.lock().await;
        guard.reset_session();
        guard.snapshot_session()
    };
    let new_id = store
        .create_new_session(&snapshot)
        .map_err(|e| format!("failed to create new session: {e}"))?;
    state.active_session = Some(new_id.clone());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Created { session_id: new_id }),
    );
    Ok(())
}

async fn runtime_session_resume(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    session_id: &str,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let Some(store) = state.session_store.as_ref() else {
        return Err("session store is unavailable".to_string());
    };

    if let Some(active_id) = state.active_session.as_deref() {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|e| format!("failed to persist session {active_id}: {e}"))?;
    }

    let snapshot = store
        .load(session_id)
        .map_err(|e| format!("failed to load session {session_id}: {e}"))?;
    {
        let mut guard = agent.lock().await;
        guard.restore_session(snapshot.clone());
    }
    store
        .save(session_id, &snapshot)
        .map_err(|e| format!("failed to refresh session {session_id}: {e}"))?;
    state.active_session = Some(session_id.to_string());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Resumed {
            session_id: session_id.to_string(),
        }),
    );
    Ok(())
}

async fn runtime_session_compact(
    agent: &Arc<Mutex<Agent>>,
    state: &mut RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) -> Result<(), String> {
    let report = {
        let mut guard = agent.lock().await;
        guard.compact_history()
    };

    let Some(report) = report else {
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Warning(WarningEvent {
                task: None,
                message: "nothing to compact; history is already focused on recent turns"
                    .to_string(),
            }),
        );
        return Ok(());
    };

    if let (Some(store), Some(active_id)) = (
        state.session_store.as_ref(),
        state.active_session.as_deref(),
    ) {
        let snapshot = agent.lock().await.snapshot_session();
        store
            .save(active_id, &snapshot)
            .map_err(|err| format!("failed to persist compacted session {active_id}: {err}"))?;
    }

    let session_id = state
        .active_session
        .clone()
        .unwrap_or_else(|| "default".to_string());
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Session(SessionEvent::Compacted {
            session_id: session_id.clone(),
        }),
    );
    emit_event(
        event_tx,
        seq,
        RuntimeEvent::Warning(WarningEvent {
            task: None,
            message: format!(
                "compacted session {session_id}: removed {} turn(s), {} message(s) (estimated {} -> {})",
                report.removed_turns,
                report.removed_messages,
                report.estimated_before,
                report.estimated_after
            ),
        }),
    );

    Ok(())
}

async fn persist_active_session_snapshot(
    agent: &Arc<Mutex<Agent>>,
    state: &RuntimeActorState,
    event_tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
) {
    let Some(store) = state.session_store.as_ref() else {
        return;
    };
    let Some(active_session) = state.active_session.as_deref() else {
        return;
    };

    let snapshot = agent.lock().await.snapshot_session();
    if store.save(active_session, &snapshot).is_ok() {
        emit_event(
            event_tx,
            seq,
            RuntimeEvent::Session(SessionEvent::Saved {
                session_id: active_session.to_string(),
            }),
        );
    }
}

fn truncate_preview(text: &str, max_len: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    truncate_with_suffix_by_chars(&flat, max_len, "...")
}

fn api_mode_label(protocol: ApiProtocol, auth: AuthMode) -> String {
    format!("{protocol:?}/{auth:?}").to_ascii_lowercase()
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ModelClient;
    use crate::config::{ApiProtocol, AuthMode, Config};
    use crate::error::ApiError;
    use crate::tools::shell::ShellApprovalBroker;
    use crate::types::{ChatRequest, ChatResponse, Choice, Message, Role, Usage};
    use async_trait::async_trait;
    use serde_json::{json, Value};
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;
    use tokio::time::timeout;

    #[test]
    fn envelope_serializes_with_required_fields() {
        let envelope =
            RuntimeEventEnvelope::new(7, RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStarted));
        let value = serde_json::to_value(&envelope).expect("serialize");
        assert_eq!(value["seq"], json!(7));
        assert!(value["ts_unix_ms"].as_u64().is_some());
        assert_eq!(value["event"]["type"], json!("Lifecycle"));
    }

    #[test]
    fn runtime_command_round_trip_json() {
        let cmd = RuntimeCommand::SubmitPrompt {
            prompt: "list files".to_string(),
            metadata: PromptMetadata {
                source: Some("cli".to_string()),
                correlation_id: Some("abc-123".to_string()),
            },
        };

        let raw = serde_json::to_string(&cmd).expect("serialize");
        let parsed: RuntimeCommand = serde_json::from_str(&raw).expect("deserialize");
        assert_eq!(parsed, cmd);
    }

    #[test]
    fn adapter_maps_warning_event() {
        let event = AgentUiEvent::Warning {
            task_id: 3,
            message: "hello".to_string(),
        };
        let mapped = runtime_event_from_agent_ui(event);
        assert_eq!(
            mapped,
            RuntimeEvent::Warning(WarningEvent {
                task: Some(TaskRef::from_task_id(3)),
                message: "hello".to_string(),
            })
        );
    }

    #[test]
    fn adapter_maps_token_usage_event() {
        let event = AgentUiEvent::TokenUsage {
            task_id: 9,
            prompt_tokens: 12,
            completion_tokens: 7,
            session_total: 120,
        };
        let mapped = runtime_event_from_agent_ui(event);
        assert_eq!(
            mapped,
            RuntimeEvent::Metrics(MetricsEvent::TokenUsage {
                task: TaskRef::from_task_id(9),
                prompt_tokens: 12,
                completion_tokens: 7,
                session_total_tokens: 120,
            })
        );
    }

    #[test]
    fn adapter_maps_tool_events() {
        let call = AgentUiEvent::ToolCall {
            task_id: 1,
            name: "run_shell".to_string(),
            args: "{\"command\":\"ls\"}".to_string(),
        };
        let mapped_call = runtime_event_from_agent_ui(call);
        assert_eq!(
            mapped_call,
            RuntimeEvent::Tool(ToolEvent::CallRequested {
                task: TaskRef::from_task_id(1),
                name: "run_shell".to_string(),
                arguments_json: "{\"command\":\"ls\"}".to_string(),
            })
        );

        let result = AgentUiEvent::ToolResult {
            task_id: 1,
            name: "run_shell".to_string(),
            args: "{}".to_string(),
            result: "exit code: 0".to_string(),
        };
        let mapped_result = runtime_event_from_agent_ui(result);
        assert_eq!(
            mapped_result,
            RuntimeEvent::Tool(ToolEvent::Result {
                task: TaskRef::from_task_id(1),
                name: "run_shell".to_string(),
                arguments_json: "{}".to_string(),
                result: "exit code: 0".to_string(),
            })
        );
    }

    #[test]
    fn adapter_maps_reasoning_event() {
        let event = AgentUiEvent::ReasoningTrace {
            task_id: 42,
            field: "reasoning_stream".to_string(),
            trace: "step-1".to_string(),
        };
        let mapped = runtime_event_from_agent_ui(event);
        assert_eq!(
            mapped,
            RuntimeEvent::Model(ModelEvent::ReasoningDelta {
                task: TaskRef::from_task_id(42),
                field: "reasoning_stream".to_string(),
                delta: "step-1".to_string(),
            })
        );
    }

    #[test]
    fn envelope_builder_preserves_sequence_and_wraps_event() {
        let envelope = runtime_envelope_from_agent_ui(
            11,
            AgentUiEvent::Warning {
                task_id: 8,
                message: "check".to_string(),
            },
        );
        assert_eq!(envelope.seq, 11);
        assert!(envelope.ts_unix_ms > 0);
        assert_eq!(
            envelope.event,
            RuntimeEvent::Warning(WarningEvent {
                task: Some(TaskRef::from_task_id(8)),
                message: "check".to_string(),
            })
        );
    }

    #[test]
    fn warning_event_omits_task_when_absent() {
        let event = RuntimeEvent::Warning(WarningEvent {
            task: None,
            message: "plain warning".to_string(),
        });
        let value = serde_json::to_value(&event).expect("serialize");
        let payload: &Value = value.get("payload").expect("payload");
        assert!(payload.get("task").is_none());
    }

    struct MockClient {
        responses: StdMutex<VecDeque<ChatResponse>>,
        delay: Option<Duration>,
    }

    impl MockClient {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                delay: None,
            }
        }

        fn with_delay(responses: Vec<ChatResponse>, delay: Duration) -> Self {
            Self {
                responses: StdMutex::new(responses.into()),
                delay: Some(delay),
            }
        }
    }

    #[async_trait]
    impl ModelClient for MockClient {
        async fn chat(&self, _request: &ChatRequest) -> Result<ChatResponse, ApiError> {
            if let Some(delay) = self.delay {
                tokio::time::sleep(delay).await;
            }
            self.responses
                .lock()
                .expect("lock")
                .pop_front()
                .ok_or_else(|| ApiError::InvalidResponse("no mock response queued".to_string()))
        }
    }

    fn chat_response_text(id: &str, text: &str) -> ChatResponse {
        ChatResponse {
            id: id.to_string(),
            choices: vec![Choice {
                index: 0,
                message: Message {
                    role: Role::Assistant,
                    content: Some(text.to_string()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    extra: BTreeMap::new(),
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            }),
        }
    }

    async fn recv_event(rx: &mut RuntimeEventStream) -> RuntimeEvent {
        timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("event timeout")
            .expect("event channel closed")
            .event
    }

    #[tokio::test]
    async fn runtime_actor_submit_prompt_emits_expected_events() {
        let agent = Agent::with_client(
            Config::default(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::new(vec![chat_response_text("r1", "ok")])),
        );
        let (handle, mut events) =
            spawn_runtime_with_agent(agent, Config::default(), None, None, None);

        // startup lifecycle events
        assert!(matches!(
            recv_event(&mut events).await,
            RuntimeEvent::Lifecycle(LifecycleEvent::RuntimeStarted)
        ));
        assert!(matches!(
            recv_event(&mut events).await,
            RuntimeEvent::Lifecycle(LifecycleEvent::ConfigLoaded)
        ));

        handle
            .send(RuntimeCommand::SubmitPrompt {
                prompt: "ping".to_string(),
                metadata: PromptMetadata::default(),
            })
            .await
            .expect("send");

        let mut labels = Vec::new();
        for _ in 0..9 {
            let event = recv_event(&mut events).await;
            let label = match event {
                RuntimeEvent::Task(TaskEvent::Queued { .. }) => "queued",
                RuntimeEvent::Task(TaskEvent::Started { .. }) => "started",
                RuntimeEvent::Metrics(MetricsEvent::ContextUsage { .. }) => "context",
                RuntimeEvent::Model(ModelEvent::RequestStarted { .. }) => "request",
                RuntimeEvent::Metrics(MetricsEvent::TokenUsage { .. }) => "tokens",
                RuntimeEvent::Model(ModelEvent::MessageFinal { .. }) => "final",
                RuntimeEvent::Task(TaskEvent::Completed { .. }) => {
                    labels.push("completed".to_string());
                    break;
                }
                _ => "other",
            };
            labels.push(label.to_string());
        }

        assert_eq!(
            labels,
            vec![
                "queued",
                "started",
                "context",
                "request",
                "tokens",
                "final",
                "completed"
            ]
        );
    }

    #[tokio::test]
    async fn runtime_actor_cancel_task_emits_cancelling() {
        let agent = Agent::with_client(
            Config::default(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::with_delay(
                vec![chat_response_text("r1", "slow")],
                Duration::from_millis(300),
            )),
        );
        let (handle, mut events) =
            spawn_runtime_with_agent(agent, Config::default(), None, None, None);
        let _ = recv_event(&mut events).await;
        let _ = recv_event(&mut events).await;

        handle
            .send(RuntimeCommand::SubmitPrompt {
                prompt: "slow".to_string(),
                metadata: PromptMetadata::default(),
            })
            .await
            .expect("send submit");
        handle
            .send(RuntimeCommand::CancelTask { task_id: 1 })
            .await
            .expect("send cancel");

        let mut saw_cancelling = false;
        let mut saw_completed = false;
        for _ in 0..10 {
            match recv_event(&mut events).await {
                RuntimeEvent::Task(TaskEvent::Cancelling { task }) => {
                    saw_cancelling = task.task_id == 1;
                }
                RuntimeEvent::Task(TaskEvent::Completed { task }) => {
                    if task.task_id == 1 {
                        saw_completed = true;
                        break;
                    }
                }
                _ => {}
            }
        }

        assert!(saw_cancelling);
        assert!(saw_completed);
    }

    #[tokio::test]
    async fn runtime_actor_handles_approval_command_flow() {
        let agent = Agent::with_client(
            Config::default(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::with_delay(
                vec![chat_response_text("r1", "ok")],
                Duration::from_millis(250),
            )),
        );
        let (broker, approval_rx) = ShellApprovalBroker::channel();
        let (handle, mut events) =
            spawn_runtime_with_agent(agent, Config::default(), None, None, Some(approval_rx));
        let _ = recv_event(&mut events).await;
        let _ = recv_event(&mut events).await;

        handle
            .send(RuntimeCommand::SubmitPrompt {
                prompt: "slow".to_string(),
                metadata: PromptMetadata::default(),
            })
            .await
            .expect("send submit");

        let waiter = tokio::spawn(async move { broker.request("echo hi".to_string()).await });

        let mut approval_id = String::new();
        for _ in 0..10 {
            match recv_event(&mut events).await {
                RuntimeEvent::Task(TaskEvent::WaitingApproval {
                    task,
                    approval_id: id,
                    ..
                }) => {
                    assert_eq!(task.task_id, 1);
                    approval_id = id;
                    break;
                }
                _ => {}
            }
        }
        assert!(
            !approval_id.is_empty(),
            "runtime did not emit waiting-approval event"
        );

        handle
            .send(RuntimeCommand::Approve {
                approval_id,
                decision: ApprovalDecision::Approve,
            })
            .await
            .expect("send approve");

        let approved = waiter
            .await
            .expect("join should succeed")
            .expect("decision");
        assert!(approved);
    }

    #[tokio::test]
    async fn runtime_actor_switch_model_emits_profile_switched() {
        let mut cfg = Config::default();
        cfg.display.show_tokens = false;
        let agent = Agent::with_client(
            cfg.clone(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::new(vec![chat_response_text("r1", "ok")])),
        );
        let (handle, mut events) = spawn_runtime_with_agent(agent, cfg, None, None, None);
        let _ = recv_event(&mut events).await;
        let _ = recv_event(&mut events).await;

        handle
            .send(RuntimeCommand::SwitchModel {
                profile: "openrouter-deepseek".to_string(),
            })
            .await
            .expect("send switch");

        let mut saw_switch = false;
        let mut saw_mode_warning = false;
        for _ in 0..2 {
            match recv_event(&mut events).await {
                RuntimeEvent::Model(ModelEvent::ProfileSwitched {
                    profile, api, auth, ..
                }) => {
                    assert_eq!(profile, "openrouter-deepseek");
                    assert_eq!(api, ApiProtocol::Completions);
                    assert_eq!(auth, AuthMode::ApiKey);
                    saw_switch = true;
                }
                RuntimeEvent::Warning(WarningEvent { message, .. }) => {
                    if message.contains("model switch changed API mode") {
                        saw_mode_warning = true;
                    }
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        assert!(saw_switch, "missing switched-profile event");
        assert!(saw_mode_warning, "missing mode-switch warning");
    }

    #[tokio::test]
    async fn runtime_actor_session_compact_emits_compacted_event() {
        let mut cfg = Config::default();
        cfg.api.context_limit = Some(220);
        let mut agent = Agent::with_client(
            cfg.clone(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::new(vec![chat_response_text("r1", "ok")])),
        );

        let mut snapshot = agent.snapshot_session();
        snapshot.messages = vec![Message::system("system prompt")];
        for idx in 0..7 {
            snapshot.messages.push(Message::user(format!(
                "user turn {idx}: {}",
                "x".repeat(140)
            )));
            snapshot.messages.push(Message {
                role: Role::Assistant,
                content: Some(format!("assistant turn {idx}: {}", "y".repeat(120))),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                extra: BTreeMap::new(),
            });
        }
        agent.restore_session(snapshot);

        let (handle, mut events) =
            spawn_runtime_with_agent(agent, cfg, None, Some("demo-session".to_string()), None);
        let _ = recv_event(&mut events).await;
        let _ = recv_event(&mut events).await;

        handle
            .send(RuntimeCommand::SessionCompact)
            .await
            .expect("send compact");

        let mut saw_compacted = false;
        let mut saw_warning = false;
        for _ in 0..2 {
            match recv_event(&mut events).await {
                RuntimeEvent::Session(SessionEvent::Compacted { session_id }) => {
                    saw_compacted = session_id == "demo-session";
                }
                RuntimeEvent::Warning(WarningEvent { message, .. }) => {
                    if message.contains("compacted session demo-session") {
                        saw_warning = true;
                    }
                }
                _ => {}
            }
        }

        assert!(saw_compacted, "missing compacted session event");
        assert!(saw_warning, "missing compaction summary warning");
    }
}
