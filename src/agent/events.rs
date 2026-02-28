//! Agent event plumbing.
//!
//! This module isolates the bridging logic between core agent operations and
//! UI/runtime event sinks so the main loop can focus on request/tool flow.

use super::Agent;
use crate::runtime::{
    ModelEvent, RuntimeEvent, RuntimeEventEnvelope, TaskRef, ToolEvent, WarningEvent,
};
use crate::tools::ToolStreamEvent;
use tokio::sync::mpsc;

/// Background UI events emitted by an agent running in background mode.
#[derive(Debug, Clone)]
pub enum AgentUiEvent {
    /// Non-fatal warning message emitted for a task.
    Warning {
        /// Task identifier that produced the warning.
        task_id: u64,
        /// Warning text suitable for user display.
        message: String,
    },
    /// Token accounting update for a completed model request.
    TokenUsage {
        /// Task identifier associated with the token usage.
        task_id: u64,
        /// Prompt tokens consumed in the latest request.
        prompt_tokens: u64,
        /// Completion tokens consumed in the latest request.
        completion_tokens: u64,
        /// Rolling prompt+completion total for the current session.
        session_total: u64,
    },
    /// Reasoning/thinking trace update emitted by provider-specific fields.
    ReasoningTrace {
        /// Task identifier associated with the trace.
        task_id: u64,
        /// Source field name for the trace payload.
        field: String,
        /// Extracted reasoning text.
        trace: String,
    },
    /// Tool call notification emitted before tool execution.
    ToolCall {
        /// Task identifier associated with the tool call.
        task_id: u64,
        /// Tool name requested by the model.
        name: String,
        /// Raw tool arguments JSON.
        args: String,
    },
    /// Tool result notification emitted after tool execution.
    ToolResult {
        /// Task identifier associated with the tool result.
        task_id: u64,
        /// Tool name that produced the result.
        name: String,
        /// Raw tool arguments JSON used for execution.
        args: String,
        /// Raw tool output payload.
        result: String,
    },
}

impl Agent {
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

    /// Return the active task id from either UI or runtime sink binding.
    pub(super) fn current_task_id(&self) -> Option<u64> {
        // Prefer task id from legacy UI sink when present; otherwise fall back
        // to runtime sink so helpers can work in either embedding mode.
        self.live_output_sink
            .as_ref()
            .map(|(task_id, _)| *task_id)
            .or_else(|| {
                self.runtime_event_sink
                    .as_ref()
                    .map(|(task_id, _)| *task_id)
            })
    }

    /// Return the active runtime task reference when a runtime sink is bound.
    pub(super) fn current_task_ref(&self) -> Option<TaskRef> {
        self.runtime_event_sink
            .as_ref()
            .map(|(task_id, _)| TaskRef::from_task_id(*task_id))
    }

    /// Emit a legacy UI event if a UI sink is configured.
    pub(super) fn emit_ui_event(&mut self, event: AgentUiEvent) -> bool {
        self.live_output_sink
            .as_ref()
            .is_some_and(|(_, tx)| tx.send(event).is_ok())
    }

    /// Emit a runtime event envelope if a runtime sink is configured.
    pub(super) fn emit_runtime_event(&mut self, event: RuntimeEvent) -> bool {
        let Some((_, tx)) = &self.runtime_event_sink else {
            return false;
        };
        // Sequence values are generated here so downstream consumers receive a
        // monotonic stream regardless of source event type.
        let envelope = RuntimeEventEnvelope::new(self.runtime_event_seq, event);
        self.runtime_event_seq = self.runtime_event_seq.saturating_add(1);
        tx.send(envelope).is_ok()
    }

    /// Emit a warning through runtime/UI sinks and renderer as appropriate.
    pub(super) fn warn_live(&mut self, msg: &str) {
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
            let _ = self.emit_ui_event(AgentUiEvent::Warning {
                task_id,
                message: msg.to_string(),
            });
            // In runtime/background mode the CLI consumes RuntimeEvent stream, so avoid
            // duplicate direct stderr rendering from the embedded renderer.
            return;
        }
        self.renderer.warn(msg);
    }

    /// Emit a token usage update to the active sink (runtime stream or renderer).
    pub(super) fn token_usage_live(&mut self, prompt: u64, completion: u64, session_total: u64) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.token_usage(prompt, completion, session_total);
                return;
            };
            let _ = self.emit_ui_event(AgentUiEvent::TokenUsage {
                task_id,
                prompt_tokens: prompt,
                completion_tokens: completion,
                session_total,
            });
            return;
        }
        self.renderer.token_usage(prompt, completion, session_total);
    }

    /// Emit provider reasoning traces to runtime and optional legacy UI sinks.
    pub(super) fn reasoning_trace_live(&mut self, field: &str, trace: &str) {
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
            let _ = self.emit_ui_event(AgentUiEvent::ReasoningTrace {
                task_id,
                field: field.to_string(),
                trace: trace.to_string(),
            });
            return;
        }
        self.renderer.reasoning_trace(field, trace);
    }

    /// Emit tool call start notification to the active sink.
    pub(super) fn tool_call_live(&mut self, name: &str, args: &str) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.tool_call(name, args);
                return;
            };
            let _ = self.emit_ui_event(AgentUiEvent::ToolCall {
                task_id,
                name: name.to_string(),
                args: args.to_string(),
            });
            return;
        }
        self.renderer.tool_call(name, args);
    }

    /// Emit tool result notification to the active sink.
    pub(super) fn tool_result_live(&mut self, name: &str, args: &str, result: &str) {
        if self.suppress_live_output {
            let Some(task_id) = self.current_task_id() else {
                self.renderer.tool_result(result);
                return;
            };
            let _ = self.emit_ui_event(AgentUiEvent::ToolResult {
                task_id,
                name: name.to_string(),
                args: args.to_string(),
                result: result.to_string(),
            });
            return;
        }
        self.renderer.tool_result(result);
    }

    /// Adapt streamed tool execution events into runtime `ToolEvent` variants.
    pub(super) fn emit_tool_stream_event(&mut self, tool_name: &str, event: ToolStreamEvent) {
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
}
