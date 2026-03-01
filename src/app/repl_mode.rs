//! Interactive REPL mode orchestration.
//!
//! This module owns the long-running runtime/event/input loop used by default
//! `buddy` invocations and keeps that logic out of `app::entry::run`.

use crate::app::approval::{
    approval_prompt_actor, deny_pending_approval, render_shell_approval_request,
    send_approval_decision,
};
use crate::app::commands::model::handle_model_command;
use crate::app::commands::session::{handle_session_command, initialize_active_session};
use crate::app::commands::theme::handle_theme_command;
use crate::app::repl_loop::{
    dispatch_shared_slash_action, SharedSlashDispatchContext, SharedSlashDispatchMode,
    SharedSlashDispatchOutcome,
};
use crate::app::startup::{render_session_startup_line, render_startup_banner};
use crate::app::tasks::{
    background_liveness_line, collect_runtime_events, drain_completed_tasks, enforce_task_timeouts,
    process_runtime_events, ProcessRuntimeEventsContext,
};
use buddy::agent::Agent;
use buddy::config::default_history_path;
use buddy::config::Config;
use buddy::repl::{
    approval_policy_label, has_elapsed_timeouts, mark_task_running, parse_approval_decision,
    task_is_waiting_for_approval, ApprovalPolicy, BackgroundTask, CompletedBackgroundTask,
    PendingApproval, ResumeRequest, RuntimeContextState,
};
use buddy::runtime::{spawn_runtime_with_shared_agent, PromptMetadata, RuntimeCommand};
use buddy::session::{default_uses_legacy_root, SessionStore};
use buddy::tokens::TokenTracker;
use buddy::tools::execution::ExecutionContext;
use buddy::tools::shell::ShellApprovalRequest;
use buddy::ui::render::{set_progress_enabled, RenderSink, Renderer};
use buddy::ui::terminal as term_ui;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Warning shown when commands requiring an idle loop are requested during background work.
const BACKGROUND_TASK_WARNING: &str =
    "Background tasks are in progress. Allowed commands now: /ps, /kill <id>, /timeout <dur> [id], /approve <mode>, /status, /context.";

/// Inputs required to start interactive REPL mode.
pub(crate) struct ReplModeInputs<'a> {
    /// Terminal renderer implementation for status/output.
    pub renderer: &'a Renderer,
    /// Parsed CLI arguments used by prompt/execution targeting.
    pub cli_args: &'a crate::cli::Args,
    /// Effective runtime configuration.
    pub config: Config,
    /// Prepared execution context (local/ssh/container + optional tmux).
    pub execution: ExecutionContext,
    /// Whether capture-pane/send-keys tools are available in this context.
    pub capture_pane_enabled: bool,
    /// Bootstrapped agent instance.
    pub agent: Agent,
    /// Optional startup resume request from CLI command.
    pub resume_request: Option<ResumeRequest>,
    /// Shell approval request stream (present when tool confirmations are enabled).
    pub shell_approval_rx: Option<mpsc::UnboundedReceiver<ShellApprovalRequest>>,
}

/// Run the interactive REPL loop until EOF/cancel/quit.
pub(crate) async fn run_repl_mode(inputs: ReplModeInputs<'_>) -> i32 {
    // REPL-mode walkthrough:
    // 1) initialize session/history/runtime wiring,
    // 2) drain/render runtime events and completed tasks,
    // 3) service approval prompts with command support,
    // 4) service normal prompt input and slash commands,
    // 5) persist history and shutdown runtime on exit.
    let ReplModeInputs {
        renderer,
        cli_args,
        mut config,
        execution,
        capture_pane_enabled,
        mut agent,
        resume_request,
        mut shell_approval_rx,
    } = inputs;

    if default_uses_legacy_root() {
        renderer.warn(
            "Using deprecated `.agentx/` session root; migrate to `.buddyx/` before legacy support is removed after v0.4.",
        );
    }
    let session_store = match SessionStore::open_default() {
        Ok(store) => store,
        Err(err) => {
            renderer.error(&err);
            return 1;
        }
    };
    let (startup_session_state, mut active_session) =
        match initialize_active_session(renderer, &session_store, &mut agent, resume_request) {
            Ok(value) => value,
            Err(msg) => {
                renderer.error(&msg);
                return 1;
            }
        };

    render_startup_banner(
        config.display.color,
        &config.api.model,
        execution.tmux_attach_info().as_ref(),
    );
    render_session_startup_line(
        config.display.color,
        startup_session_state,
        &active_session,
        context_used_percent(&agent).unwrap_or(0),
    );

    let mut repl_state = term_ui::ReplState::default();
    let history_path = if config.display.persist_history {
        default_history_path()
    } else {
        None
    };
    if let Some(path) = history_path.as_ref() {
        if let Err(err) = repl_state.load_history_file(path) {
            renderer.warn(&format!(
                "failed to load command history from {}: {err}",
                path.display()
            ));
        }
    }

    let agent = Arc::new(Mutex::new(agent));
    let (runtime, mut runtime_events) = spawn_runtime_with_shared_agent(
        Arc::clone(&agent),
        config.clone(),
        Some(session_store.clone()),
        Some(active_session.clone()),
        shell_approval_rx.take(),
    );
    let mut background_tasks: Vec<BackgroundTask> = Vec::new();
    let mut completed_tasks: Vec<CompletedBackgroundTask> = Vec::new();
    let mut pending_approval: Option<PendingApproval> = None;
    let mut approval_policy = ApprovalPolicy::Ask;
    let mut pending_runtime_events = Vec::new();
    let mut runtime_context =
        RuntimeContextState::new(config.api.context_limit.map(|limit| limit as u64));
    let mut last_prompt_context_used_percent: Option<u16> = None;

    loop {
        // Keep runtime-derived state fresh before each prompt cycle.
        enforce_task_timeouts(
            renderer,
            &runtime,
            &mut background_tasks,
            &mut pending_approval,
        )
        .await;
        collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
        let mut runtime_event_context = ProcessRuntimeEventsContext {
            renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut pending_runtime_events, &mut runtime_event_context);
        let _ = drain_completed_tasks(renderer, &mut completed_tasks);
        if background_tasks.is_empty() {
            set_progress_enabled(true);
        }

        if let Some(approval) = pending_approval.take() {
            // Approval input mode temporarily replaces normal prompt handling.
            let approval_actor = approval_prompt_actor(
                cli_args.ssh.as_deref(),
                cli_args.container.as_deref(),
                execution.tmux_attach_info().as_ref(),
            );
            render_shell_approval_request(
                config.display.color,
                renderer,
                &approval_actor,
                &approval.command,
                approval.risk.as_deref(),
                approval.why.as_deref(),
            );
            let approval_prompt = term_ui::ApprovalPrompt {
                actor: &approval_actor,
                command: &approval.command,
                privileged: approval.privesc.unwrap_or(false),
                mutation: approval.mutation.unwrap_or(false),
            };

            let approval_input = match term_ui::read_repl_line_with_interrupt(
                config.display.color,
                &mut repl_state,
                cli_args.ssh.as_deref(),
                None,
                term_ui::PromptMode::Approval,
                Some(&approval_prompt),
                || term_ui::ReadPoll {
                    interrupt: has_elapsed_timeouts(&background_tasks),
                    status_line: None,
                },
            ) {
                Ok(term_ui::ReadOutcome::Line(line)) => line,
                Ok(term_ui::ReadOutcome::Eof) => {
                    deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                    continue;
                }
                Ok(term_ui::ReadOutcome::Cancelled) => {
                    deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                    break;
                }
                Ok(term_ui::ReadOutcome::Interrupted) => {
                    pending_approval = Some(approval);
                    continue;
                }
                Err(err) => {
                    renderer.error(&format!("failed to read approval input: {err}"));
                    deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                    continue;
                }
            };

            let approval_input = approval_input.trim();
            eprintln!();
            if let Some(decision) = parse_approval_decision(approval_input) {
                // Direct y/n responses resolve the pending runtime approval.
                let task_id = approval.task_id;
                if let Err(err) = send_approval_decision(&runtime, &approval, decision).await {
                    renderer.warn(&err);
                } else {
                    mark_task_running(&mut background_tasks, task_id);
                }
                continue;
            }

            if let Some(action) = term_ui::parse_slash_command(approval_input) {
                // Approval prompt supports a restricted slash-command subset.
                match &action {
                    term_ui::SlashCommandAction::Status => {
                        let guard = agent.try_lock().ok();
                        render_status(
                            renderer,
                            &config,
                            guard.as_deref(),
                            runtime_context,
                            &background_tasks,
                            approval_policy,
                            capture_pane_enabled,
                        );
                        pending_approval = Some(approval);
                        continue;
                    }
                    term_ui::SlashCommandAction::Context => {
                        let guard = agent.try_lock().ok();
                        render_context(
                            renderer,
                            guard.as_deref(),
                            runtime_context,
                            &background_tasks,
                        );
                        pending_approval = Some(approval);
                        continue;
                    }
                    _ => {}
                }
                let mut dispatch_context = SharedSlashDispatchContext {
                    runtime: &runtime,
                    background_tasks: background_tasks.as_mut_slice(),
                    pending_approval: &mut pending_approval,
                    active_approval: Some(&approval),
                    approval_policy: &mut approval_policy,
                    mode: SharedSlashDispatchMode::Approval {
                        task_id: approval.task_id,
                    },
                };
                let dispatch_outcome =
                    dispatch_shared_slash_action(renderer, &action, &mut dispatch_context).await;
                match dispatch_outcome {
                    SharedSlashDispatchOutcome::Unhandled => {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                        if task_is_waiting_for_approval(&background_tasks, approval.task_id) {
                            pending_approval = Some(approval);
                        }
                    }
                    SharedSlashDispatchOutcome::RequeueApproval => {
                        pending_approval = Some(approval);
                    }
                    SharedSlashDispatchOutcome::Handled
                    | SharedSlashDispatchOutcome::ApprovalResolved => {}
                }
                continue;
            }

            renderer.warn(
                "Approval required. Reply with y/yes or n/no. You can also use /ps, /kill <id>, /timeout <dur> [id], /approve <mode>, /status, /context, /compact.",
            );
            pending_approval = Some(approval);
            continue;
        }

        if let Some(latest) = agent
            .try_lock()
            .ok()
            .and_then(|guard| context_used_percent(&guard))
        {
            last_prompt_context_used_percent = Some(latest);
        } else if runtime_context.context_limit > 0 {
            last_prompt_context_used_percent =
                Some(display_context_percent(runtime_context.used_percent as f64));
        }

        let input = match term_ui::read_repl_line_with_interrupt(
            config.display.color,
            &mut repl_state,
            cli_args.ssh.as_deref(),
            last_prompt_context_used_percent,
            term_ui::PromptMode::Normal,
            None,
            || {
                let has_new_runtime_events =
                    collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
                term_ui::ReadPoll {
                    interrupt: has_new_runtime_events || has_elapsed_timeouts(&background_tasks),
                    status_line: background_liveness_line(&background_tasks),
                }
            },
        ) {
            Ok(term_ui::ReadOutcome::Line(line)) => line,
            Ok(term_ui::ReadOutcome::Eof) => break,
            Ok(term_ui::ReadOutcome::Cancelled) => break,
            Ok(term_ui::ReadOutcome::Interrupted) => continue,
            Err(err) => {
                renderer.error(&format!("failed to read input: {err}"));
                break;
            }
        };

        let input = input.trim_end();
        if input.trim().is_empty() {
            continue;
        }

        collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
        let mut runtime_event_context = ProcessRuntimeEventsContext {
            renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut pending_runtime_events, &mut runtime_event_context);
        let _ = drain_completed_tasks(renderer, &mut completed_tasks);

        repl_state.push_history(input);
        let has_background_tasks = !background_tasks.is_empty();

        if let Some(action) = term_ui::parse_slash_command(input) {
            // Normal prompt slash-command dispatch, with shared handlers first.
            match &action {
                term_ui::SlashCommandAction::Status => {
                    let guard = agent.try_lock().ok();
                    render_status(
                        renderer,
                        &config,
                        guard.as_deref(),
                        runtime_context,
                        &background_tasks,
                        approval_policy,
                        capture_pane_enabled,
                    );
                    continue;
                }
                term_ui::SlashCommandAction::Context => {
                    let guard = agent.try_lock().ok();
                    render_context(
                        renderer,
                        guard.as_deref(),
                        runtime_context,
                        &background_tasks,
                    );
                    continue;
                }
                _ => {}
            }

            let mut dispatch_context = SharedSlashDispatchContext {
                runtime: &runtime,
                background_tasks: background_tasks.as_mut_slice(),
                pending_approval: &mut pending_approval,
                active_approval: None,
                approval_policy: &mut approval_policy,
                mode: SharedSlashDispatchMode::Repl,
            };
            let dispatch_outcome =
                dispatch_shared_slash_action(renderer, &action, &mut dispatch_context).await;
            if !matches!(dispatch_outcome, SharedSlashDispatchOutcome::Unhandled) {
                continue;
            }

            match action {
                term_ui::SlashCommandAction::Quit => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        break;
                    }
                }
                term_ui::SlashCommandAction::Session { verb, name } => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        handle_session_command(
                            renderer,
                            &session_store,
                            &runtime,
                            &mut active_session,
                            verb.as_deref(),
                            name.as_deref(),
                        )
                        .await;
                    }
                }
                term_ui::SlashCommandAction::Compact => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else if let Err(err) = runtime.send(RuntimeCommand::SessionCompact).await {
                        renderer.warn(&format!("failed to submit session compact command: {err}"));
                    }
                }
                term_ui::SlashCommandAction::Model(selector) => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        handle_model_command(renderer, &mut config, &runtime, selector.as_deref())
                            .await;
                    }
                }
                term_ui::SlashCommandAction::Theme(selector) => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        handle_theme_command(
                            renderer,
                            &mut config,
                            cli_args.config.as_deref(),
                            selector.as_deref(),
                        );
                    }
                }
                term_ui::SlashCommandAction::Login(selector) => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else if let Err(msg) = crate::app::entry::run_login_flow(
                        renderer,
                        &config,
                        selector.as_deref(),
                        false,
                        false,
                    )
                    .await
                    {
                        renderer.warn(&msg);
                    }
                }
                term_ui::SlashCommandAction::Help => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        render_help(renderer);
                    }
                }
                term_ui::SlashCommandAction::Unknown(cmd) => {
                    if has_background_tasks {
                        renderer.warn(BACKGROUND_TASK_WARNING);
                    } else {
                        renderer.warn(&format!("Unknown slash command: {cmd}. Try /help."));
                    }
                }
                term_ui::SlashCommandAction::Ps
                | term_ui::SlashCommandAction::Kill(_)
                | term_ui::SlashCommandAction::Timeout { .. }
                | term_ui::SlashCommandAction::Approve(_) => {}
                term_ui::SlashCommandAction::Status | term_ui::SlashCommandAction::Context => {}
            }
            continue;
        }

        if has_background_tasks {
            renderer.warn(BACKGROUND_TASK_WARNING);
            continue;
        }

        // Submit user prompt as a background runtime task.
        set_progress_enabled(false);
        if let Err(err) = runtime
            .send(RuntimeCommand::SubmitPrompt {
                prompt: input.to_string(),
                metadata: PromptMetadata {
                    source: Some("repl".to_string()),
                    correlation_id: None,
                },
            })
            .await
        {
            renderer.error(&format!("failed to start background task: {err}"));
        }
    }

    if let Some(path) = history_path.as_ref() {
        if let Err(err) = repl_state.save_history_file(path) {
            renderer.warn(&format!(
                "failed to save command history to {}: {err}",
                path.display()
            ));
        }
    }
    let _ = runtime.send(RuntimeCommand::Shutdown).await;
    0
}

/// Render `/help` slash-command listing.
fn render_help(renderer: &dyn RenderSink) {
    renderer.section("slash commands");
    for cmd in &term_ui::SLASH_COMMANDS {
        renderer.field(cmd.name, cmd.description);
    }
    eprintln!();
}

/// Render `/status` output for config/runtime/task metadata.
fn render_status(
    renderer: &dyn RenderSink,
    config: &Config,
    agent: Option<&Agent>,
    runtime_context: RuntimeContextState,
    background_tasks: &[BackgroundTask],
    approval_policy: ApprovalPolicy,
    capture_pane_enabled: bool,
) {
    renderer.section("status");
    renderer.field("model_profile", &config.agent.model);
    renderer.field("model", &config.api.model);
    renderer.field("base_url", &config.api.base_url);
    renderer.field("api", &format!("{:?}", config.api.protocol));
    renderer.field("auth", &format!("{:?}", config.api.auth));
    renderer.field("theme", &config.display.theme);
    renderer.field("max_iterations", &config.agent.max_iterations.to_string());
    renderer.field("tools", &enabled_tools(config, capture_pane_enabled));
    renderer.field("background_tasks", &background_tasks.len().to_string());
    renderer.field("approval_policy", &approval_policy_label(approval_policy));

    if let Some(agent) = agent {
        renderer.field("context_limit", &agent.tracker().context_limit.to_string());
        renderer.field("messages", &agent.messages().len().to_string());
        renderer.field(
            "session_tokens",
            &agent.tracker().session_total().to_string(),
        );
    } else {
        let context_limit = if runtime_context.context_limit == 0 {
            "auto".to_string()
        } else {
            runtime_context.context_limit.to_string()
        };
        renderer.field("context_limit", &context_limit);
        renderer.field("messages", "busy (task in progress)");
        renderer.field(
            "session_tokens",
            &runtime_context.session_total_tokens.to_string(),
        );
    }

    eprintln!();
}

/// Render `/context` output from either live agent state or runtime snapshot.
fn render_context(
    renderer: &dyn RenderSink,
    agent: Option<&Agent>,
    runtime_context: RuntimeContextState,
    background_tasks: &[BackgroundTask],
) {
    renderer.section("context");
    renderer.field("background_tasks", &background_tasks.len().to_string());

    if let Some(agent) = agent {
        let tracker = agent.tracker();
        let estimated = TokenTracker::estimate_messages(agent.messages());
        let percent = if tracker.context_limit == 0 {
            0.0
        } else {
            (estimated as f64 / tracker.context_limit as f64) * 100.0
        };

        renderer.field(
            "window_estimate",
            &format!(
                "{estimated} / {} tokens ({percent:.1}%)",
                tracker.context_limit
            ),
        );
        renderer.field(
            "last_call",
            &format!(
                "prompt:{} completion:{}",
                tracker.last_prompt_tokens, tracker.last_completion_tokens
            ),
        );
        renderer.field("session_total", &tracker.session_total().to_string());
        renderer.field("messages", &agent.messages().len().to_string());
    } else {
        if runtime_context.context_limit == 0 {
            renderer.field("window_estimate", "unknown (context limit auto)");
        } else {
            renderer.field(
                "window_estimate",
                &format!(
                    "{} / {} tokens ({:.1}%)",
                    runtime_context.estimated_tokens,
                    runtime_context.context_limit,
                    runtime_context.used_percent
                ),
            );
        }
        renderer.field(
            "last_call",
            &format!(
                "prompt:{} completion:{}",
                runtime_context.last_prompt_tokens, runtime_context.last_completion_tokens
            ),
        );
        renderer.field(
            "session_total",
            &runtime_context.session_total_tokens.to_string(),
        );
        renderer.field("messages", "busy (task in progress)");
    }

    eprintln!();
}

/// Return enabled tool names as a printable comma-separated string.
fn enabled_tools(config: &Config, capture_pane_enabled: bool) -> String {
    let tools = enabled_tool_names(config, capture_pane_enabled);
    if tools.is_empty() {
        "none".to_string()
    } else {
        tools.join(", ")
    }
}

/// Return enabled tool identifiers in deterministic display order.
fn enabled_tool_names(config: &Config, capture_pane_enabled: bool) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if config.tools.shell_enabled {
        tools.push("run_shell");
    }
    if capture_pane_enabled {
        tools.push("capture-pane");
        tools.push("send-keys");
    }
    if config.tools.fetch_enabled {
        tools.push("fetch_url");
    }
    if config.tools.files_enabled {
        tools.push("read_file");
        tools.push("write_file");
    }
    if config.tools.search_enabled {
        tools.push("web_search");
    }
    tools.push("time");
    tools
}

/// Estimate current context-window usage percentage from live agent state.
fn context_used_percent(agent: &Agent) -> Option<u16> {
    let tracker = agent.tracker();
    if tracker.context_limit == 0 {
        return None;
    }
    let estimated = TokenTracker::estimate_messages(agent.messages());
    let percent = (estimated as f64 / tracker.context_limit as f64) * 100.0;
    Some(display_context_percent(percent))
}

/// Convert floating-point context usage into a clamped user-facing percentage.
fn display_context_percent(percent: f64) -> u16 {
    if percent.is_nan() || percent <= 0.0 {
        return 0;
    }
    let rounded = percent.round().clamp(0.0, 9_999.0) as u16;
    if rounded == 0 {
        1
    } else {
        rounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::tools::ToolRegistry;
    use buddy::types::Message;

    #[test]
    fn context_used_percent_rounds_and_handles_zero_limit() {
        // Percentage helper should produce non-zero usage and honor zero-limit sentinel.
        let mut cfg = Config::default();
        cfg.api.context_limit = Some(100);
        let mut agent = Agent::new(cfg, ToolRegistry::new());
        let mut snapshot = agent.snapshot_session();
        snapshot.messages.push(Message::user("a".repeat(100)));
        agent.restore_session(snapshot);
        let used = context_used_percent(&agent).expect("has context limit");
        assert!(used > 0);

        let mut cfg_zero = Config::default();
        cfg_zero.api.context_limit = Some(0);
        let agent_zero = Agent::new(cfg_zero, ToolRegistry::new());
        assert_eq!(context_used_percent(&agent_zero), None);
    }
}
