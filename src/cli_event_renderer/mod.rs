//! CLI runtime-event renderer adapter.
//!
//! This module translates typed runtime events into terminal rendering updates.
//! Keeping the mapping here lets alternate frontends reuse the same runtime
//! stream without depending on `main.rs` orchestration details.

mod handlers;

use buddy::config::Config;
use buddy::render::RenderSink;
use buddy::runtime::{RuntimeEvent, RuntimeEventEnvelope};

use crate::repl_support::{
    BackgroundTask, CompletedBackgroundTask, PendingApproval, RuntimeContextState,
};

/// Mutable render-time state mirrored from the interactive loop.
pub(crate) struct RuntimeEventRenderContext<'a> {
    pub renderer: &'a dyn RenderSink,
    pub background_tasks: &'a mut Vec<BackgroundTask>,
    pub completed_tasks: &'a mut Vec<CompletedBackgroundTask>,
    pub pending_approval: &'a mut Option<PendingApproval>,
    pub config: &'a mut Config,
    pub active_session: &'a mut String,
    pub runtime_context: &'a mut RuntimeContextState,
}

/// Consume queued runtime events and update render/runtime state.
pub(crate) fn process_runtime_events(
    events: &mut Vec<RuntimeEventEnvelope>,
    ctx: &mut RuntimeEventRenderContext<'_>,
) {
    for envelope in events.drain(..) {
        match envelope.event {
            RuntimeEvent::Lifecycle(_) => {}
            RuntimeEvent::Warning(event) => handlers::warning::handle_warning(ctx, event),
            RuntimeEvent::Error(event) => handlers::warning::handle_error(ctx, event),
            RuntimeEvent::Session(event) => handlers::session::handle_session(ctx, event),
            RuntimeEvent::Task(event) => handlers::task::handle_task(ctx, event),
            RuntimeEvent::Model(event) => handlers::model::handle_model(ctx, event),
            RuntimeEvent::Tool(event) => handlers::tool::handle_tool(ctx, event),
            RuntimeEvent::Metrics(event) => handlers::metrics::handle_metrics(ctx, event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::render::{ProgressHandle, ProgressMetrics, Renderer};
    use buddy::runtime::{TaskEvent, TaskRef, ToolEvent, WarningEvent};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct MockRenderer {
        entries: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl MockRenderer {
        fn record(&self, kind: &str, message: &str) {
            self.entries
                .lock()
                .expect("mock renderer lock")
                .push((kind.to_string(), message.to_string()));
        }

        fn saw(&self, kind: &str, needle: &str) -> bool {
            self.entries
                .lock()
                .expect("mock renderer lock")
                .iter()
                .any(|(k, msg)| k == kind && msg.contains(needle))
        }
    }

    impl RenderSink for MockRenderer {
        fn prompt(&self) {}

        fn assistant_message(&self, content: &str) {
            self.record("assistant", content);
        }

        fn progress(&self, label: &str) -> ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn progress_with_metrics(&self, label: &str, _metrics: ProgressMetrics) -> ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn header(&self, model: &str) {
            self.record("header", model);
        }

        fn tool_call(&self, name: &str, args: &str) {
            self.record("tool_call", &format!("{name}({args})"));
        }

        fn tool_result(&self, result: &str) {
            self.record("tool_result", result);
        }

        fn token_usage(&self, prompt: u64, completion: u64, session_total: u64) {
            self.record(
                "token_usage",
                &format!("{prompt}/{completion}/{session_total}"),
            );
        }

        fn reasoning_trace(&self, field: &str, trace: &str) {
            self.record("reasoning", &format!("{field}:{trace}"));
        }

        fn warn(&self, msg: &str) {
            self.record("warn", msg);
        }

        fn section(&self, title: &str) {
            self.record("section", title);
        }

        fn activity(&self, text: &str) {
            self.record("activity", text);
        }

        fn field(&self, key: &str, value: &str) {
            self.record("field", &format!("{key}:{value}"));
        }

        fn detail(&self, text: &str) {
            self.record("detail", text);
        }

        fn error(&self, msg: &str) {
            self.record("error", msg);
        }

        fn tool_output_block(&self, text: &str, _syntax_path: Option<&str>) {
            self.record("tool_output", text);
        }

        fn command_output_block(&self, text: &str) {
            self.record("command_output", text);
        }

        fn reasoning_block(&self, text: &str) {
            self.record("reasoning_block", text);
        }

        fn approval_block(&self, text: &str) {
            self.record("approval_block", text);
        }
    }

    #[test]
    fn reducer_transitions_task_into_and_out_of_waiting_approval() {
        let renderer = MockRenderer::default();
        let mut events = vec![
            RuntimeEventEnvelope {
                seq: 1,
                ts_unix_ms: 1,
                event: RuntimeEvent::Task(TaskEvent::Queued {
                    task: TaskRef::from_task_id(7),
                    kind: "prompt".to_string(),
                    details: "demo".to_string(),
                }),
            },
            RuntimeEventEnvelope {
                seq: 2,
                ts_unix_ms: 2,
                event: RuntimeEvent::Task(TaskEvent::WaitingApproval {
                    task: TaskRef::from_task_id(7),
                    approval_id: "approve-7".to_string(),
                    command: "ls -la".to_string(),
                    risk: Some("low".to_string()),
                    mutation: Some(false),
                    privesc: Some(false),
                    why: Some("inspect files".to_string()),
                }),
            },
        ];
        let mut background_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut pending_approval = None;
        let mut config = Config::default();
        let mut active_session = "session-x".to_string();
        let mut runtime_context = RuntimeContextState::new(None);
        let mut ctx = RuntimeEventRenderContext {
            renderer: &renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut events, &mut ctx);

        assert_eq!(ctx.background_tasks.len(), 1);
        assert!(matches!(
            ctx.background_tasks[0].state,
            crate::repl_support::BackgroundTaskState::WaitingApproval { .. }
        ));
        assert_eq!(
            ctx.pending_approval
                .as_ref()
                .map(|a| a.approval_id.as_str()),
            Some("approve-7")
        );

        let mut completed_event = vec![RuntimeEventEnvelope {
            seq: 3,
            ts_unix_ms: 3,
            event: RuntimeEvent::Task(TaskEvent::Completed {
                task: TaskRef::from_task_id(7),
            }),
        }];
        process_runtime_events(&mut completed_event, &mut ctx);

        assert!(ctx.pending_approval.is_none());
        assert!(ctx.background_tasks.is_empty());
        assert_eq!(ctx.completed_tasks.len(), 1);
    }

    #[test]
    fn reducer_suppresses_transient_approval_warnings() {
        let renderer = MockRenderer::default();
        let mut events = vec![
            RuntimeEventEnvelope {
                seq: 1,
                ts_unix_ms: 1,
                event: RuntimeEvent::Warning(WarningEvent {
                    task: Some(TaskRef::from_task_id(2)),
                    message: "approval granted".to_string(),
                }),
            },
            RuntimeEventEnvelope {
                seq: 2,
                ts_unix_ms: 2,
                event: RuntimeEvent::Warning(WarningEvent {
                    task: None,
                    message: "keep this warning".to_string(),
                }),
            },
        ];
        let mut background_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut pending_approval = None;
        let mut config = Config::default();
        let mut active_session = "session-x".to_string();
        let mut runtime_context = RuntimeContextState::new(None);
        let mut ctx = RuntimeEventRenderContext {
            renderer: &renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut events, &mut ctx);

        assert!(!renderer.saw("warn", "approval granted"));
        assert!(renderer.saw("warn", "keep this warning"));
    }

    #[test]
    fn reducer_renders_tool_result_branches_without_duplicate_run_shell_chatter() {
        let renderer = MockRenderer::default();
        let shell_result = serde_json::json!({
            "result": {
                "exit_code": 0,
                "stdout": "line-a\nline-b",
                "stderr": ""
            }
        })
        .to_string();
        let mut events = vec![
            RuntimeEventEnvelope {
                seq: 1,
                ts_unix_ms: 1,
                event: RuntimeEvent::Tool(ToolEvent::CallStarted {
                    task: TaskRef::from_task_id(4),
                    name: "run_shell".to_string(),
                    detail: "ls -la".to_string(),
                }),
            },
            RuntimeEventEnvelope {
                seq: 2,
                ts_unix_ms: 2,
                event: RuntimeEvent::Tool(ToolEvent::Result {
                    task: TaskRef::from_task_id(4),
                    name: "run_shell".to_string(),
                    arguments_json: "{}".to_string(),
                    result: shell_result,
                }),
            },
            RuntimeEventEnvelope {
                seq: 3,
                ts_unix_ms: 3,
                event: RuntimeEvent::Tool(ToolEvent::Result {
                    task: TaskRef::from_task_id(5),
                    name: "read_file".to_string(),
                    arguments_json: "{\"path\":\"README.md\"}".to_string(),
                    result: "hello".to_string(),
                }),
            },
        ];
        let mut background_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut pending_approval = None;
        let mut config = Config::default();
        let mut active_session = "session-x".to_string();
        let mut runtime_context = RuntimeContextState::new(None);
        let mut ctx = RuntimeEventRenderContext {
            renderer: &renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut events, &mut ctx);

        assert!(!renderer.saw("activity", "running run_shell"));
        assert!(renderer.saw("activity", "task #4 exited with code 0"));
        assert!(renderer.saw("command_output", "line-a"));
        assert!(renderer.saw("activity", "task #5 read README.md"));
        assert!(renderer.saw("tool_output", "hello"));
    }
}
