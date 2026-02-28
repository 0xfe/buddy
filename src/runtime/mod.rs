//! Streaming runtime actor and schema facade.
//!
//! Runtime command/event types live in `schema`, while this module hosts the
//! actor orchestration and re-exports the public runtime API.

use crate::agent::Agent;
use crate::config::{select_model_profile, ApiProtocol, AuthMode, Config};
use crate::preflight::validate_active_profile_ready;
use crate::session::SessionStore;
use crate::textutil::truncate_with_suffix_by_chars;
use crate::tools::shell::ShellApprovalRequest;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{mpsc, watch, Mutex};

mod approvals;
mod schema;
mod sessions;
mod tasks;

use approvals::{
    active_approval_decision, deny_pending_approvals_for_task, handle_approval_request,
    resolve_pending_approval, PendingRuntimeApproval,
};
pub use schema::*;
use sessions::{
    persist_active_session_snapshot, runtime_session_compact, runtime_session_new,
    runtime_session_resume,
};
use tasks::{spawn_prompt_task, ActiveTask, TaskDone};

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
        let mut failed_tasks = HashSet::<u64>::new();
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
                    let mut command_ctx = RuntimeCommandContext {
                        agent: &agent,
                        state: &mut state,
                        active_task: &mut active_task,
                        next_task_id: &mut next_task_id,
                        pending_approvals: &mut pending_approvals,
                        event_tx: &event_tx,
                        seq: &mut seq,
                        agent_event_tx: &agent_event_tx,
                        task_done_tx: &task_done_tx,
                    };
                    let should_stop = handle_runtime_command(command, &mut command_ctx).await;
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
                    if let RuntimeEvent::Task(TaskEvent::Failed { task, .. }) = &agent_envelope.event {
                        if !failed_tasks.insert(task.task_id) {
                            continue;
                        }
                    }
                    emit_event(&event_tx, &mut seq, agent_envelope.event);
                }
                Some(done) = task_done_rx.recv() => {
                    if active_task.as_ref().is_some_and(|active| active.task_id == done.task_id) {
                        active_task = None;
                    }
                    deny_pending_approvals_for_task(done.task_id, &mut pending_approvals);

                    persist_active_session_snapshot(&agent, &state, &event_tx, &mut seq).await;

                    if let Err(err) = done.result {
                        if !failed_tasks.insert(done.task_id) {
                            continue;
                        }
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

fn emit_event(
    tx: &mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &mut u64,
    event: RuntimeEvent,
) {
    let _ = tx.send(RuntimeEventEnvelope::new(*seq, event));
    *seq = seq.saturating_add(1);
}

struct RuntimeCommandContext<'a> {
    agent: &'a Arc<Mutex<Agent>>,
    state: &'a mut RuntimeActorState,
    active_task: &'a mut Option<ActiveTask>,
    next_task_id: &'a mut u64,
    pending_approvals: &'a mut HashMap<String, PendingRuntimeApproval>,
    event_tx: &'a mpsc::UnboundedSender<RuntimeEventEnvelope>,
    seq: &'a mut u64,
    agent_event_tx: &'a mpsc::UnboundedSender<RuntimeEventEnvelope>,
    task_done_tx: &'a mpsc::UnboundedSender<TaskDone>,
}

async fn handle_runtime_command(
    command: RuntimeCommand,
    ctx: &mut RuntimeCommandContext<'_>,
) -> bool {
    let agent = ctx.agent;
    let state = &mut *ctx.state;
    let active_task = &mut *ctx.active_task;
    let next_task_id = &mut *ctx.next_task_id;
    let pending_approvals = &mut *ctx.pending_approvals;
    let event_tx = ctx.event_tx;
    let seq = &mut *ctx.seq;
    let agent_event_tx = ctx.agent_event_tx;
    let task_done_tx = ctx.task_done_tx;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentUiEvent;
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

        let waiter = tokio::spawn(async move { broker.request("echo hi".to_string(), None).await });

        let mut approval_id = String::new();
        for _ in 0..10 {
            if let RuntimeEvent::Task(TaskEvent::WaitingApproval {
                task,
                approval_id: id,
                ..
            }) = recv_event(&mut events).await
            {
                assert_eq!(task.task_id, 1);
                approval_id = id;
                break;
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
    async fn runtime_actor_emits_single_started_event_when_approval_resolves() {
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

        let waiter = tokio::spawn(async move { broker.request("echo hi".to_string(), None).await });

        let mut approval_id = String::new();
        let mut started_count = 0usize;
        for _ in 0..12 {
            match recv_event(&mut events).await {
                RuntimeEvent::Task(TaskEvent::Started { task }) if task.task_id == 1 => {
                    started_count += 1;
                }
                RuntimeEvent::Task(TaskEvent::WaitingApproval {
                    task,
                    approval_id: id,
                    ..
                }) if task.task_id == 1 => {
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

        for _ in 0..12 {
            match recv_event(&mut events).await {
                RuntimeEvent::Task(TaskEvent::Started { task }) if task.task_id == 1 => {
                    started_count += 1;
                }
                RuntimeEvent::Task(TaskEvent::Completed { task }) if task.task_id == 1 => break,
                _ => {}
            }
        }

        assert_eq!(started_count, 1, "expected exactly one started event");
    }

    #[tokio::test]
    async fn runtime_actor_deduplicates_failed_event_from_task_done_path() {
        // No queued response forces the mock client to return an API error, which
        // already causes the agent to emit TaskEvent::Failed.
        let agent = Agent::with_client(
            Config::default(),
            crate::tools::ToolRegistry::new(),
            Box::new(MockClient::new(vec![])),
        );
        let (handle, mut events) =
            spawn_runtime_with_agent(agent, Config::default(), None, None, None);
        let _ = recv_event(&mut events).await;
        let _ = recv_event(&mut events).await;

        handle
            .send(RuntimeCommand::SubmitPrompt {
                prompt: "trigger failure".to_string(),
                metadata: PromptMetadata::default(),
            })
            .await
            .expect("send submit");

        let mut failed_count = 0usize;
        for _ in 0..20 {
            let maybe_event = timeout(Duration::from_millis(250), events.recv()).await;
            let Ok(Some(envelope)) = maybe_event else {
                break;
            };
            if let RuntimeEvent::Task(TaskEvent::Failed { task, .. }) = envelope.event {
                if task.task_id == 1 {
                    failed_count += 1;
                }
            }
        }

        assert_eq!(failed_count, 1, "expected exactly one failed event");
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
