//! CLI runtime-event renderer adapter.
//!
//! This module translates typed runtime events into terminal rendering updates.
//! Keeping the mapping here lets alternate frontends reuse the same runtime
//! stream without depending on `main.rs` orchestration details.

use buddy::config::{select_model_profile, Config};
use buddy::render::{set_progress_enabled, RenderSink};
use buddy::runtime::{
    MetricsEvent, ModelEvent, RuntimeEvent, RuntimeEventEnvelope, TaskEvent, ToolEvent,
};

use crate::{
    mark_task_running, mark_task_waiting_for_approval, parse_shell_tool_result, parse_tool_arg,
    quote_preview, truncate_preview, BackgroundTask, BackgroundTaskState, CompletedBackgroundTask,
    PendingApproval, RuntimeContextState,
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
            RuntimeEvent::Warning(event) => {
                if is_transient_approval_warning(&event.message) {
                    continue;
                }
                if let Some(task) = event.task {
                    ctx.renderer
                        .warn(&format!("[task #{}] {}", task.task_id, event.message));
                } else {
                    ctx.renderer.warn(&event.message);
                }
            }
            RuntimeEvent::Error(event) => {
                if let Some(task) = event.task {
                    ctx.renderer
                        .error(&format!("[task #{}] {}", task.task_id, event.message));
                } else {
                    ctx.renderer.error(&event.message);
                }
            }
            RuntimeEvent::Session(event) => match event {
                buddy::runtime::SessionEvent::Created { session_id } => {
                    *ctx.active_session = session_id.clone();
                    ctx.renderer
                        .section(&format!("created session: {session_id}"));
                    eprintln!();
                }
                buddy::runtime::SessionEvent::Resumed { session_id } => {
                    *ctx.active_session = session_id.clone();
                    ctx.renderer
                        .section(&format!("resumed session: {session_id}"));
                    eprintln!();
                }
                buddy::runtime::SessionEvent::Compacted { session_id } => {
                    ctx.renderer
                        .section(&format!("compacted session: {session_id}"));
                    eprintln!();
                }
                buddy::runtime::SessionEvent::Saved { .. } => {}
            },
            RuntimeEvent::Task(event) => match event {
                TaskEvent::Queued {
                    task,
                    kind,
                    details,
                } => {
                    ctx.background_tasks.push(BackgroundTask {
                        id: task.task_id,
                        kind,
                        details,
                        started_at: std::time::Instant::now(),
                        state: BackgroundTaskState::Running,
                        timeout_at: None,
                        final_response: None,
                    });
                    set_progress_enabled(false);
                }
                TaskEvent::Started { task } => {
                    mark_task_running(ctx.background_tasks, task.task_id);
                }
                TaskEvent::WaitingApproval {
                    task,
                    approval_id,
                    command,
                } => {
                    if mark_task_waiting_for_approval(
                        ctx.background_tasks,
                        task.task_id,
                        &command,
                        &approval_id,
                    ) && ctx.pending_approval.is_none()
                    {
                        *ctx.pending_approval = Some(PendingApproval {
                            task_id: task.task_id,
                            approval_id,
                            command,
                        });
                    }
                }
                TaskEvent::Cancelling { task } => {
                    if let Some(bg) = ctx
                        .background_tasks
                        .iter_mut()
                        .find(|bg| bg.id == task.task_id)
                    {
                        bg.state = BackgroundTaskState::Cancelling {
                            since: std::time::Instant::now(),
                        };
                    }
                }
                TaskEvent::Completed { task } => {
                    if ctx
                        .pending_approval
                        .as_ref()
                        .is_some_and(|approval| approval.task_id == task.task_id)
                    {
                        *ctx.pending_approval = None;
                    }
                    if let Some(index) = ctx
                        .background_tasks
                        .iter()
                        .position(|bg| bg.id == task.task_id)
                    {
                        let task = ctx.background_tasks.swap_remove(index);
                        ctx.completed_tasks.push(CompletedBackgroundTask {
                            id: task.id,
                            kind: task.kind,
                            started_at: task.started_at,
                            result: Ok(task.final_response.unwrap_or_default()),
                        });
                    }
                }
                TaskEvent::Failed { task, message } => {
                    if ctx
                        .pending_approval
                        .as_ref()
                        .is_some_and(|approval| approval.task_id == task.task_id)
                    {
                        *ctx.pending_approval = None;
                    }
                    if let Some(index) = ctx
                        .background_tasks
                        .iter()
                        .position(|bg| bg.id == task.task_id)
                    {
                        let task = ctx.background_tasks.swap_remove(index);
                        ctx.completed_tasks.push(CompletedBackgroundTask {
                            id: task.id,
                            kind: task.kind,
                            started_at: task.started_at,
                            result: Err(message),
                        });
                    }
                }
            },
            RuntimeEvent::Model(event) => match event {
                ModelEvent::ReasoningDelta { task, field, delta } => {
                    ctx.renderer
                        .reasoning_trace(&format!("task #{} {field}", task.task_id), &delta);
                }
                ModelEvent::MessageFinal { task, content } => {
                    if let Some(bg) = ctx
                        .background_tasks
                        .iter_mut()
                        .find(|bg| bg.id == task.task_id)
                    {
                        bg.final_response = Some(content);
                    }
                }
                ModelEvent::ProfileSwitched {
                    profile,
                    model,
                    base_url,
                    api,
                    auth,
                } => {
                    if let Err(err) = select_model_profile(ctx.config, &profile) {
                        ctx.renderer.warn(&format!(
                            "runtime switched model profile `{profile}`, but local config sync failed: {err}"
                        ));
                    }
                    ctx.renderer
                        .section(&format!("switched model profile: {profile}"));
                    ctx.renderer.field("model", &model);
                    ctx.renderer.field("base_url", &base_url);
                    ctx.renderer
                        .field("api", &format!("{api:?}").to_ascii_lowercase());
                    ctx.renderer
                        .field("auth", &format!("{auth:?}").to_ascii_lowercase());
                    ctx.renderer.field(
                        "context_limit",
                        &ctx.config
                            .api
                            .context_limit
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "auto".to_string()),
                    );
                    eprintln!();
                }
                ModelEvent::RequestStarted { .. } | ModelEvent::TextDelta { .. } => {}
            },
            RuntimeEvent::Tool(event) => match event {
                ToolEvent::CallRequested { .. } => {}
                ToolEvent::CallStarted { task, name, detail } => {
                    if name == "run_shell" {
                        // Shell commands already get explicit approval/request rendering (when
                        // enabled) and a structured final result block. Suppress duplicate
                        // "running run_shell" lifecycle chatter.
                        continue;
                    }
                    ctx.renderer.activity(&format!(
                        "task #{} running {name}: {}",
                        task.task_id,
                        truncate_preview(&detail, 120)
                    ));
                }
                ToolEvent::StdoutChunk { task, name, chunk } => {
                    if name == "run_shell" {
                        // For run_shell we render stdout/stderr from the final ToolEvent::Result
                        // payload so output only appears once.
                        continue;
                    } else {
                        ctx.renderer.activity(&format!(
                            "task #{} {name} output: {}",
                            task.task_id,
                            truncate_preview(&chunk, 120)
                        ));
                    }
                }
                ToolEvent::StderrChunk { task, name, chunk } => {
                    if name == "run_shell" {
                        continue;
                    }
                    ctx.renderer
                        .activity(&format!("task #{} {name} stderr:", task.task_id));
                    ctx.renderer.command_output_block(&chunk);
                }
                ToolEvent::Info {
                    task,
                    name,
                    message,
                } => {
                    if name == "run_shell" {
                        continue;
                    }
                    ctx.renderer.activity(&format!(
                        "task #{} {name}: {}",
                        task.task_id,
                        truncate_preview(&message, 120)
                    ));
                }
                ToolEvent::Completed { task, name, detail } => {
                    if name == "run_shell" {
                        continue;
                    }
                    ctx.renderer.activity(&format!(
                        "task #{} {name}: {}",
                        task.task_id,
                        truncate_preview(&detail, 120)
                    ));
                }
                ToolEvent::Result {
                    task,
                    name,
                    arguments_json,
                    result,
                } => {
                    render_tool_result(ctx.renderer, task.task_id, &name, &arguments_json, &result)
                }
            },
            RuntimeEvent::Metrics(event) => match event {
                MetricsEvent::TokenUsage {
                    task,
                    prompt_tokens,
                    completion_tokens,
                    session_total_tokens,
                } => {
                    ctx.runtime_context.last_prompt_tokens = prompt_tokens;
                    ctx.runtime_context.last_completion_tokens = completion_tokens;
                    ctx.runtime_context.session_total_tokens = session_total_tokens;
                    ctx.renderer.section("task");
                    ctx.renderer.field(
                        "tokens",
                        &format!(
                            "#{} prompt:{prompt_tokens} completion:{completion_tokens} session:{session_total_tokens}",
                            task.task_id
                        ),
                    );
                    eprintln!();
                }
                MetricsEvent::ContextUsage {
                    estimated_tokens,
                    context_limit,
                    used_percent,
                    ..
                } => {
                    ctx.runtime_context.estimated_tokens = estimated_tokens;
                    ctx.runtime_context.context_limit = context_limit;
                    ctx.runtime_context.used_percent = used_percent;
                }
                MetricsEvent::PhaseDuration { .. } => {}
            },
        }
    }
}

fn render_tool_result(renderer: &dyn RenderSink, task_id: u64, name: &str, args: &str, result: &str) {
    match name {
        "run_shell" => {
            if let Some(shell) = parse_shell_tool_result(result) {
                renderer.activity(&format!(
                    "task #{task_id} exited with code {}",
                    shell.exit_code
                ));
                if !shell.stdout.trim().is_empty() {
                    renderer.command_output_block(&shell.stdout);
                }
                if !shell.stderr.trim().is_empty() {
                    renderer.detail("stderr:");
                    renderer.command_output_block(&shell.stderr);
                }
                return;
            }
            if result.contains("command dispatched to tmux pane") {
                renderer.activity(&format!(
                    "task #{task_id} run_shell: {}",
                    truncate_preview(result, 140)
                ));
                eprintln!();
                return;
            }
        }
        "read_file" => {
            let path = parse_tool_arg(args, "path").unwrap_or_else(|| "<path>".to_string());
            renderer.activity(&format!("task #{task_id} read {path}"));
            renderer.tool_output_block(result, Some(path.as_str()));
            return;
        }
        "write_file" => {
            renderer.activity(&format!(
                "task #{task_id} write_file: {}",
                truncate_preview(result, 120)
            ));
            eprintln!();
            return;
        }
        "fetch_url" => {
            let url = parse_tool_arg(args, "url").unwrap_or_else(|| "<url>".to_string());
            renderer.activity(&format!(
                "task #{task_id} fetched {url}: \"{}\"",
                quote_preview(result, 120)
            ));
            eprintln!();
            return;
        }
        "web_search" => {
            let query = parse_tool_arg(args, "query").unwrap_or_else(|| "<query>".to_string());
            renderer.activity(&format!(
                "task #{task_id} searched \"{}\": \"{}\"",
                truncate_preview(&query, 64),
                quote_preview(result, 120)
            ));
            eprintln!();
            return;
        }
        "capture-pane" => {
            let target = parse_tool_arg(args, "target").unwrap_or_else(|| "<default>".to_string());
            renderer.activity(&format!("task #{task_id} captured pane {target}"));
            renderer.command_output_block(result);
            return;
        }
        "time" => {
            renderer.activity(&format!(
                "task #{task_id} read harness time: \"{}\"",
                quote_preview(result, 120)
            ));
            eprintln!();
            return;
        }
        "send-keys" => {
            renderer.activity(&format!(
                "task #{task_id} send-keys: {}",
                truncate_preview(result, 120)
            ));
            eprintln!();
            return;
        }
        _ => {}
    }

    renderer.activity(&format!(
        "task #{task_id} {name}: {}",
        truncate_preview(result, 120)
    ));
    eprintln!();
}

fn is_transient_approval_warning(message: &str) -> bool {
    matches!(
        message.trim().to_ascii_lowercase().as_str(),
        "approval granted" | "approval denied"
    )
}
