//! CLI entry point for buddy.

mod cli;

use buddy::agent::{Agent, AgentUiEvent};
use buddy::auth::{
    complete_openai_device_login, load_provider_tokens, login_provider_key_for_base_url,
    save_provider_tokens, start_openai_device_login, supports_openai_login, try_open_browser,
};
use buddy::config::ensure_default_global_config;
use buddy::config::load_config;
use buddy::config::select_model_profile;
use buddy::config::{AuthMode, Config};
use buddy::prompt::{render_system_prompt, ExecutionTarget, SystemPromptParams};
use buddy::render::Renderer;
use buddy::session::{SessionStore, SessionSummary};
use buddy::tokens::TokenTracker;
use buddy::tools::capture_pane::CapturePaneTool;
use buddy::tools::execution::{ExecutionContext, TmuxAttachInfo, TmuxAttachTarget};
use buddy::tools::fetch::FetchTool;
use buddy::tools::files::{ReadFileTool, WriteFileTool};
use buddy::tools::search::WebSearchTool;
use buddy::tools::send_keys::SendKeysTool;
use buddy::tools::shell::{ShellApprovalBroker, ShellApprovalRequest, ShellTool};
use buddy::tools::time::TimeTool;
use buddy::tools::ToolRegistry;
use buddy::tui as repl;
use clap::Parser;
use crossterm::style::{Color, Stylize};
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const BACKGROUND_TASK_WARNING: &str =
    "Background tasks are in progress. Allowed commands now: /ps, /kill <id>, /timeout <dur> [id], /approve <mode>, /status, /context.";

#[tokio::main]
async fn main() {
    let args = cli::Args::parse();

    if let Err(e) = ensure_default_global_config() {
        eprintln!("warning: failed to initialize ~/.config/buddy/buddy.toml: {e}");
    }

    // Load config.
    let mut config = match load_config(args.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // Apply CLI overrides.
    if let Some(model) = &args.model {
        if config.models.contains_key(model) {
            if let Err(e) = select_model_profile(&mut config, model) {
                eprintln!("error: failed to select model profile `{model}`: {e}");
                std::process::exit(1);
            }
        } else {
            // Backward-compatible behavior: if the argument is not a configured
            // profile key, treat it as a direct API model-id override.
            config.api.model = model.clone();
        }
    }
    if let Some(url) = &args.base_url {
        config.api.base_url = url.clone();
    }
    if args.no_color {
        config.display.color = false;
    }

    let renderer = Renderer::new(config.display.color);

    let resume_request = match resume_request_from_command(args.command.as_ref()) {
        Ok(request) => request,
        Err(msg) => {
            renderer.error(&msg);
            std::process::exit(1);
        }
    };

    if let Some(cli::Command::Login { model }) = args.command.as_ref() {
        let selector = model.as_deref();
        if let Err(msg) = run_login_flow(&renderer, &config, selector).await {
            renderer.error(&msg);
            std::process::exit(1);
        }
        return;
    }

    // Validate minimum config.
    if config.api.base_url.is_empty() {
        renderer.error(
            "No API base URL configured. Set models.<name>.api_base_url in buddy.toml or BUDDY_BASE_URL env var.",
        );
        std::process::exit(1);
    }

    if let Err(msg) = ensure_active_auth_ready(&config) {
        renderer.error(&msg);
        std::process::exit(1);
    }

    if (args.container.is_some() || args.ssh.is_some() || args.tmux.is_some())
        && !config.tools.shell_enabled
        && !config.tools.files_enabled
    {
        renderer.error(
            "Execution target flags (--container/--ssh/--tmux) require `run_shell` or file tools to be enabled.",
        );
        renderer.error("Enable `tools.shell_enabled` and/or `tools.files_enabled` in config.");
        std::process::exit(1);
    }

    let execution_tools_enabled = config.tools.shell_enabled || config.tools.files_enabled;
    let requested_tmux_session = args.tmux.clone().flatten();
    let execution = if !execution_tools_enabled {
        ExecutionContext::local()
    } else if let Some(container) = &args.container {
        match ExecutionContext::container_tmux(container.clone(), requested_tmux_session).await {
            Ok(ctx) => ctx,
            Err(e) => {
                renderer.error(&format!("failed to initialize container execution: {e}"));
                std::process::exit(1);
            }
        }
    } else if let Some(target) = &args.ssh {
        match ExecutionContext::ssh(target.clone(), requested_tmux_session).await {
            Ok(ctx) => ctx,
            Err(e) => {
                renderer.error(&format!("failed to initialize ssh execution: {e}"));
                std::process::exit(1);
            }
        }
    } else {
        match ExecutionContext::local_tmux(requested_tmux_session).await {
            Ok(ctx) => ctx,
            Err(e) => {
                renderer.error(&format!("failed to initialize local tmux execution: {e}"));
                std::process::exit(1);
            }
        }
    };

    let capture_pane_enabled = execution.capture_pane_available();
    let prompt_tool_names = enabled_tool_names(&config, capture_pane_enabled);

    // Render the built-in prompt template in one place with runtime parameters.
    let custom_prompt = config.agent.system_prompt.trim().to_string();
    config.agent.system_prompt = render_system_prompt(SystemPromptParams {
        execution_target: if let Some(container) = args.container.as_deref() {
            ExecutionTarget::Container(container)
        } else if let Some(host) = args.ssh.as_deref() {
            ExecutionTarget::Ssh(host)
        } else {
            ExecutionTarget::Local
        },
        enabled_tools: prompt_tool_names.clone(),
        custom_instructions: (!custom_prompt.is_empty()).then_some(custom_prompt.as_str()),
    });

    // Build tool registry from config.
    let mut tools = ToolRegistry::new();
    let interactive_mode = !matches!(args.command.as_ref(), Some(cli::Command::Exec { .. }));
    let (shell_approval_broker, mut shell_approval_rx) =
        if config.tools.shell_enabled && config.tools.shell_confirm && interactive_mode {
            let (broker, rx) = ShellApprovalBroker::channel();
            (Some(broker), Some(rx))
        } else {
            (None, None)
        };

    if config.tools.shell_enabled {
        tools.register(ShellTool {
            confirm: config.tools.shell_confirm,
            color: config.display.color,
            execution: execution.clone(),
            approval: shell_approval_broker.clone(),
        });
    }
    if capture_pane_enabled {
        tools.register(CapturePaneTool {
            execution: execution.clone(),
        });
        tools.register(SendKeysTool {
            execution: execution.clone(),
        });
    }
    if config.tools.fetch_enabled {
        tools.register(FetchTool);
    }
    if config.tools.files_enabled {
        tools.register(ReadFileTool {
            execution: execution.clone(),
        });
        tools.register(WriteFileTool {
            execution: execution.clone(),
        });
    }
    if config.tools.search_enabled {
        tools.register(WebSearchTool);
    }
    tools.register(TimeTool);

    // Create agent.
    let mut agent = Agent::new(config.clone(), tools);

    if let Some(cli::Command::Exec { prompt }) = args.command.as_ref() {
        // One-shot mode: send prompt, print response, exit.
        match agent.send(prompt).await {
            Ok(response) => {
                renderer.assistant_message(&response);
            }
            Err(e) => {
                renderer.error(&e.to_string());
                std::process::exit(1);
            }
        }
    } else {
        // Interactive REPL.
        let session_store = match SessionStore::open_default() {
            Ok(store) => store,
            Err(e) => {
                renderer.error(&e);
                std::process::exit(1);
            }
        };
        let (startup_session_state, mut active_session) = match initialize_active_session(
            &renderer,
            &session_store,
            &mut agent,
            resume_request,
        ) {
            Ok(value) => value,
            Err(msg) => {
                renderer.error(&msg);
                std::process::exit(1);
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

        let mut repl_state = repl::ReplState::default();
        let agent = Arc::new(Mutex::new(agent));
        let mut background_tasks: Vec<BackgroundTask> = Vec::new();
        let mut next_task_id: u64 = 1;
        let mut pending_approval: Option<PendingApproval> = None;
        let mut approval_policy = ApprovalPolicy::Ask;
        let (agent_ui_tx, mut agent_ui_rx) = mpsc::unbounded_channel::<AgentUiEvent>();
        let mut pending_ui_events: Vec<AgentUiEvent> = Vec::new();
        let mut last_prompt_context_used_percent: Option<u16> = None;

        loop {
            enforce_task_timeouts(&renderer, &mut background_tasks, &mut pending_approval);
            collect_agent_ui_events(&mut agent_ui_rx, &mut pending_ui_events);
            render_agent_ui_events(&renderer, &mut pending_ui_events);
            let completed_task = drain_finished_tasks(&renderer, &mut background_tasks).await;
            if completed_task {
                persist_active_session(&renderer, &session_store, &agent, &active_session).await;
            }
            enforce_task_timeouts(&renderer, &mut background_tasks, &mut pending_approval);
            collect_agent_ui_events(&mut agent_ui_rx, &mut pending_ui_events);
            render_agent_ui_events(&renderer, &mut pending_ui_events);
            if pending_approval.is_none() {
                pending_approval = collect_approval_request(
                    &renderer,
                    &mut background_tasks,
                    &mut shell_approval_rx,
                    &mut approval_policy,
                );
            }

            if let Some(approval) = pending_approval.take() {
                let approval_actor = approval_prompt_actor(
                    args.ssh.as_deref(),
                    args.container.as_deref(),
                    execution.tmux_attach_info().as_ref(),
                );
                render_shell_approval_request(&renderer, &approval_actor, &approval.command);

                let approval_input = match repl::read_repl_line_with_interrupt(
                    config.display.color,
                    &mut repl_state,
                    args.ssh.as_deref(),
                    None,
                    repl::PromptMode::Approval,
                    None,
                    || repl::ReadPoll {
                        interrupt: has_elapsed_timeouts(&background_tasks),
                        status_line: None,
                    },
                ) {
                    Ok(repl::ReadOutcome::Line(line)) => line,
                    Ok(repl::ReadOutcome::Eof) => {
                        deny_pending_approval(&mut background_tasks, approval);
                        continue;
                    }
                    Ok(repl::ReadOutcome::Cancelled) => {
                        deny_pending_approval(&mut background_tasks, approval);
                        break;
                    }
                    Ok(repl::ReadOutcome::Interrupted) => {
                        pending_approval = Some(approval);
                        continue;
                    }
                    Err(e) => {
                        renderer.error(&format!("failed to read approval input: {e}"));
                        deny_pending_approval(&mut background_tasks, approval);
                        continue;
                    }
                };

                let approval_input = approval_input.trim();
                eprintln!();
                if let Some(decision) = parse_approval_decision(approval_input) {
                    let task_id = approval.task_id;
                    match decision {
                        ApprovalDecision::Approve => {
                            mark_task_running(&mut background_tasks, task_id);
                            approval.request.approve();
                        }
                        ApprovalDecision::Deny => {
                            mark_task_running(&mut background_tasks, task_id);
                            approval.request.deny();
                        }
                    }
                    continue;
                }

                if let Some(action) = repl::parse_slash_command(approval_input) {
                    match action {
                        repl::SlashCommandAction::Status => {
                            let guard = agent.try_lock().ok();
                            render_status(
                                &renderer,
                                &config,
                                guard.as_deref(),
                                &background_tasks,
                                approval_policy,
                                capture_pane_enabled,
                            );
                        }
                        repl::SlashCommandAction::Context => {
                            let guard = agent.try_lock().ok();
                            render_context(&renderer, guard.as_deref(), &background_tasks);
                        }
                        repl::SlashCommandAction::Ps => {
                            render_background_tasks(&renderer, &background_tasks);
                        }
                        repl::SlashCommandAction::Kill(id_arg) => {
                            let Some(id_arg) = id_arg.as_deref() else {
                                renderer.warn("Usage: /kill <task-id>");
                                pending_approval = Some(approval);
                                continue;
                            };
                            let Ok(task_id) = id_arg.parse::<u64>() else {
                                renderer.warn("Task id must be a number. Usage: /kill <task-id>");
                                pending_approval = Some(approval);
                                continue;
                            };
                            kill_background_task(
                                &renderer,
                                &mut background_tasks,
                                &mut pending_approval,
                                task_id,
                            );
                        }
                        repl::SlashCommandAction::Timeout { duration, task_id } => {
                            match apply_task_timeout_command(
                                &mut background_tasks,
                                duration.as_deref(),
                                task_id.as_deref(),
                            ) {
                                Ok(msg) => {
                                    renderer.section(&msg);
                                    eprintln!();
                                }
                                Err(msg) => {
                                    renderer.warn(&msg);
                                    pending_approval = Some(approval);
                                    continue;
                                }
                            }
                        }
                        repl::SlashCommandAction::Approve(mode_arg) => {
                            let mode_arg = mode_arg.as_deref().unwrap_or("");
                            match update_approval_policy(mode_arg, &mut approval_policy) {
                                Ok(msg) => {
                                    renderer.section(&msg);
                                    eprintln!();
                                    if let Some(decision) =
                                        active_approval_decision(&mut approval_policy)
                                    {
                                        mark_task_running(&mut background_tasks, approval.task_id);
                                        match decision {
                                            ApprovalDecision::Approve => approval.request.approve(),
                                            ApprovalDecision::Deny => approval.request.deny(),
                                        }
                                        continue;
                                    }
                                }
                                Err(msg) => {
                                    renderer.warn(&msg);
                                    pending_approval = Some(approval);
                                    continue;
                                }
                            }
                        }
                        _ => renderer.warn(BACKGROUND_TASK_WARNING),
                    }

                    if pending_approval.is_none()
                        && task_is_waiting_for_approval(&background_tasks, approval.task_id)
                    {
                        pending_approval = Some(approval);
                    }
                    if pending_approval.is_none() {
                        pending_approval = collect_approval_request(
                            &renderer,
                            &mut background_tasks,
                            &mut shell_approval_rx,
                            &mut approval_policy,
                        );
                    }
                    continue;
                }

                renderer.warn(
                    "Approval required. Reply with y/yes or n/no. You can also use /ps, /kill <id>, /timeout <dur> [id], /approve <mode>, /status, /context.",
                );
                pending_approval = Some(approval);
                continue;
            }

            let mut interrupted_approval: Option<PendingApproval> = None;
            if let Some(latest) = agent
                .try_lock()
                .ok()
                .and_then(|guard| context_used_percent(&guard))
            {
                last_prompt_context_used_percent = Some(latest);
            }
            let input = match repl::read_repl_line_with_interrupt(
                config.display.color,
                &mut repl_state,
                args.ssh.as_deref(),
                last_prompt_context_used_percent,
                repl::PromptMode::Normal,
                None,
                || {
                    let mut should_interrupt = false;
                    if interrupted_approval.is_none() {
                        interrupted_approval = collect_approval_request(
                            &renderer,
                            &mut background_tasks,
                            &mut shell_approval_rx,
                            &mut approval_policy,
                        );
                    }
                    if interrupted_approval.is_some() {
                        should_interrupt = true;
                    }

                    let had_events =
                        collect_agent_ui_events(&mut agent_ui_rx, &mut pending_ui_events);
                    if had_events {
                        should_interrupt = true;
                    }

                    if has_finished_background_tasks(&background_tasks) {
                        should_interrupt = true;
                    }
                    if has_elapsed_timeouts(&background_tasks) {
                        should_interrupt = true;
                    }

                    repl::ReadPoll {
                        interrupt: should_interrupt,
                        status_line: background_liveness_line(&background_tasks),
                    }
                },
            ) {
                Ok(repl::ReadOutcome::Line(line)) => line,
                Ok(repl::ReadOutcome::Eof) => break,
                Ok(repl::ReadOutcome::Cancelled) => break,
                Ok(repl::ReadOutcome::Interrupted) => {
                    pending_approval = interrupted_approval;
                    continue;
                }
                Err(e) => {
                    renderer.error(&format!("failed to read input: {e}"));
                    break;
                }
            };

            let input = input.trim_end();
            if input.trim().is_empty() {
                continue;
            }

            // A task may complete while we are waiting for user input.
            let completed_task = drain_finished_tasks(&renderer, &mut background_tasks).await;
            if completed_task {
                persist_active_session(&renderer, &session_store, &agent, &active_session).await;
            }

            repl_state.push_history(input);
            let has_background_tasks = !background_tasks.is_empty();

            if let Some(action) = repl::parse_slash_command(input) {
                match action {
                    repl::SlashCommandAction::Quit => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            break;
                        }
                    }
                    repl::SlashCommandAction::Status => {
                        let guard = agent.try_lock().ok();
                        render_status(
                            &renderer,
                            &config,
                            guard.as_deref(),
                            &background_tasks,
                            approval_policy,
                            capture_pane_enabled,
                        );
                    }
                    repl::SlashCommandAction::Context => {
                        let guard = agent.try_lock().ok();
                        render_context(&renderer, guard.as_deref(), &background_tasks);
                    }
                    repl::SlashCommandAction::Ps => {
                        render_background_tasks(&renderer, &background_tasks);
                    }
                    repl::SlashCommandAction::Kill(id_arg) => {
                        let Some(id_arg) = id_arg.as_deref() else {
                            renderer.warn("Usage: /kill <task-id>");
                            continue;
                        };
                        let Ok(task_id) = id_arg.parse::<u64>() else {
                            renderer.warn("Task id must be a number. Usage: /kill <task-id>");
                            continue;
                        };
                        kill_background_task(
                            &renderer,
                            &mut background_tasks,
                            &mut pending_approval,
                            task_id,
                        );
                    }
                    repl::SlashCommandAction::Timeout { duration, task_id } => {
                        match apply_task_timeout_command(
                            &mut background_tasks,
                            duration.as_deref(),
                            task_id.as_deref(),
                        ) {
                            Ok(msg) => {
                                renderer.section(&msg);
                                eprintln!();
                            }
                            Err(msg) => renderer.warn(&msg),
                        }
                    }
                    repl::SlashCommandAction::Approve(mode_arg) => {
                        let mode_arg = mode_arg.as_deref().unwrap_or("");
                        match update_approval_policy(mode_arg, &mut approval_policy) {
                            Ok(msg) => {
                                renderer.section(&msg);
                                eprintln!();
                            }
                            Err(msg) => renderer.warn(&msg),
                        }
                    }
                    repl::SlashCommandAction::Session { verb, name } => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            handle_session_command(
                                &renderer,
                                &session_store,
                                &agent,
                                &mut active_session,
                                verb.as_deref(),
                                name.as_deref(),
                            )
                            .await;
                        }
                    }
                    repl::SlashCommandAction::Model(selector) => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            handle_model_command(
                                &renderer,
                                &mut config,
                                &agent,
                                selector.as_deref(),
                            )
                            .await;
                        }
                    }
                    repl::SlashCommandAction::Login(selector) => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else if let Err(msg) =
                            run_login_flow(&renderer, &config, selector.as_deref()).await
                        {
                            renderer.warn(&msg);
                        }
                    }
                    repl::SlashCommandAction::Help => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            render_help(&renderer);
                        }
                    }
                    repl::SlashCommandAction::Unknown(cmd) => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            renderer.warn(&format!("Unknown slash command: {cmd}. Try /help."));
                        }
                    }
                }
                continue;
            }

            if has_background_tasks {
                renderer.warn(BACKGROUND_TASK_WARNING);
                continue;
            }

            let task_id = next_task_id;
            next_task_id = next_task_id.saturating_add(1);
            let prompt_text = input.to_string();
            let prompt_preview = truncate_preview(input, 72);
            let agent_ref = Arc::clone(&agent);
            let agent_ui_tx = agent_ui_tx.clone();
            let (cancel_tx, cancel_rx) = watch::channel(false);
            Renderer::set_progress_enabled(false);
            let handle = tokio::spawn(async move {
                let mut agent = agent_ref.lock().await;
                agent.set_live_output_suppressed(true);
                agent.set_live_output_sink(Some((task_id, agent_ui_tx)));
                agent.set_cancellation_receiver(Some(cancel_rx));
                let result = agent.send(&prompt_text).await;
                agent.set_cancellation_receiver(None);
                agent.set_live_output_sink(None);
                agent.set_live_output_suppressed(false);
                result
            });
            background_tasks.push(BackgroundTask {
                id: task_id,
                kind: "prompt".to_string(),
                details: prompt_preview.clone(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                cancel_tx,
                handle,
            });
        }
    }
}

fn render_help(renderer: &Renderer) {
    renderer.section("slash commands");
    for cmd in &repl::SLASH_COMMANDS {
        renderer.field(cmd.name, cmd.description);
    }
    eprintln!();
}

async fn persist_active_session(
    renderer: &Renderer,
    session_store: &SessionStore,
    agent: &Arc<Mutex<Agent>>,
    active_session: &str,
) {
    let Ok(agent_guard) = agent.try_lock() else {
        return;
    };
    let snapshot = agent_guard.snapshot_session();
    drop(agent_guard);
    if let Err(e) = session_store.save(active_session, &snapshot) {
        renderer.warn(&format!("failed to persist session {active_session}: {e}"));
    }
}

async fn handle_session_command(
    renderer: &Renderer,
    session_store: &SessionStore,
    agent: &Arc<Mutex<Agent>>,
    active_session: &mut String,
    verb: Option<&str>,
    name: Option<&str>,
) {
    let action = verb.unwrap_or("list").trim().to_ascii_lowercase();
    match action.as_str() {
        "" | "list" => match session_store.list() {
            Ok(sessions) => render_sessions(renderer, active_session, &sessions),
            Err(e) => renderer.warn(&format!("failed to list sessions: {e}")),
        },
        "resume" => {
            let Some(requested_id) = name.map(str::trim).filter(|s| !s.is_empty()) else {
                renderer.warn("Usage: /session resume <session-id|last>");
                return;
            };

            let target_id = if requested_id.eq_ignore_ascii_case("last") {
                match session_store.resolve_last() {
                    Ok(Some(last)) => last,
                    Ok(None) => {
                        renderer.warn("No saved sessions found.");
                        return;
                    }
                    Err(e) => {
                        renderer.warn(&format!("failed to resolve last session: {e}"));
                        return;
                    }
                }
            } else {
                requested_id.to_string()
            };

            if target_id == *active_session {
                renderer.section(&format!("session already active: {target_id}"));
                eprintln!();
                return;
            }

            let current_snapshot = {
                let guard = agent.lock().await;
                guard.snapshot_session()
            };
            if let Err(e) = session_store.save(active_session, &current_snapshot) {
                renderer.warn(&format!("failed to persist current session: {e}"));
                return;
            }

            let snapshot = match session_store.load(&target_id) {
                Ok(snapshot) => snapshot,
                Err(e) => {
                    renderer.warn(&format!("failed to load session {target_id}: {e}"));
                    return;
                }
            };
            let snapshot_copy = snapshot.clone();
            {
                let mut guard = agent.lock().await;
                guard.restore_session(snapshot);
            }

            *active_session = target_id.clone();
            if let Err(e) = session_store.save(active_session, &snapshot_copy) {
                renderer.warn(&format!(
                    "failed to mark session {target_id} as active: {e}"
                ));
            }
            renderer.section(&format!("resumed session: {target_id}"));
            eprintln!();
        }
        "new" | "create" => {
            if name.is_some() {
                renderer.warn("Usage: /session new");
                return;
            }

            let current_snapshot = {
                let guard = agent.lock().await;
                guard.snapshot_session()
            };
            if let Err(e) = session_store.save(active_session, &current_snapshot) {
                renderer.warn(&format!("failed to persist current session: {e}"));
                return;
            }

            let new_snapshot = {
                let mut guard = agent.lock().await;
                guard.reset_session();
                guard.snapshot_session()
            };
            match session_store.create_new_session(&new_snapshot) {
                Ok(new_id) => {
                    *active_session = new_id.clone();
                    renderer.section(&format!("created session: {new_id}"));
                }
                Err(e) => {
                    renderer.warn(&format!("failed to create session: {e}"));
                    return;
                }
            }
            eprintln!();
        }
        _ => {
            renderer
                .warn("Usage: /session [list] | /session resume <session-id|last> | /session new");
        }
    }
}

async fn handle_model_command(
    renderer: &Renderer,
    config: &mut Config,
    agent: &Arc<Mutex<Agent>>,
    selector: Option<&str>,
) {
    if config.models.is_empty() {
        renderer.warn("No configured model profiles. Add `[models.<name>]` entries to buddy.toml.");
        return;
    }

    let names = configured_model_profile_names(config);
    if names.len() == 1 {
        renderer.warn("Only one model profile is configured.");
        return;
    }

    let profile_name = if let Some(selector) = selector {
        let selected_input = selector.trim();
        if selected_input.is_empty() {
            return;
        }
        match resolve_model_profile_selector(config, &names, selected_input) {
            Ok(name) => name,
            Err(msg) => {
                renderer.warn(&msg);
                return;
            }
        }
    } else {
        let options = model_picker_options(config, &names);
        let initial = names
            .iter()
            .position(|name| name == &config.agent.model)
            .unwrap_or(0);
        match repl::pick_from_list(
            config.display.color,
            "model profiles",
            "Use ↑/↓ to pick, Enter to confirm, Esc to cancel.",
            &options,
            initial,
        ) {
            Ok(Some(index)) => names[index].clone(),
            Ok(None) => return,
            Err(e) => {
                renderer.warn(&format!("failed to read model selection: {e}"));
                return;
            }
        }
    };

    if profile_name == config.agent.model {
        renderer.section(&format!("model profile already active: {profile_name}"));
        eprintln!();
        return;
    }

    let mut next_config = config.clone();
    if let Err(e) = select_model_profile(&mut next_config, &profile_name) {
        renderer.warn(&format!(
            "failed to select model profile `{profile_name}`: {e}"
        ));
        return;
    }

    if let Err(msg) = ensure_active_auth_ready(&next_config) {
        renderer.warn(&msg);
        return;
    }

    *config = next_config;

    {
        let mut guard = agent.lock().await;
        guard.switch_api_config(config.api.clone());
    }

    renderer.section(&format!("switched model profile: {profile_name}"));
    renderer.field("model", &config.api.model);
    renderer.field("base_url", &config.api.base_url);
    renderer.field(
        "context_limit",
        &config
            .api
            .context_limit
            .map(|v| v.to_string())
            .unwrap_or_else(|| "auto".to_string()),
    );
    eprintln!();
}

fn configured_model_profile_names(config: &Config) -> Vec<String> {
    config.models.keys().cloned().collect()
}

fn model_picker_options(config: &Config, names: &[String]) -> Vec<String> {
    let mut options = Vec::with_capacity(names.len());
    for (idx, name) in names.iter().enumerate() {
        let Some(profile) = config.models.get(name) else {
            continue;
        };
        let marker = if name == &config.agent.model {
            "*"
        } else {
            " "
        };
        let api_model = resolved_profile_api_model(profile, name);
        let value = format!(
            "{}.{} {} | {} | {:?} | {:?}",
            idx + 1,
            marker,
            api_model,
            profile.api_base_url.trim(),
            profile.api,
            profile.auth
        );
        options.push(value);
    }
    options
}

fn resolved_profile_api_model(profile: &buddy::config::ModelConfig, profile_name: &str) -> String {
    profile
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(profile_name)
        .to_string()
}

fn resolve_model_profile_selector(
    config: &Config,
    names: &[String],
    selector: &str,
) -> Result<String, String> {
    let trimmed = normalize_model_selector(selector);
    if trimmed.is_empty() {
        return Err("Usage: /model <name|index>".to_string());
    }

    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 || index > names.len() {
            return Err(format!(
                "Model index out of range: {index}. Choose 1-{}.",
                names.len()
            ));
        }
        return Ok(names[index - 1].clone());
    }

    if config.models.contains_key(trimmed) {
        return Ok(trimmed.to_string());
    }

    let normalized = trimmed.to_ascii_lowercase();
    let mut matches = config
        .models
        .keys()
        .filter(|name| name.to_ascii_lowercase() == normalized)
        .cloned()
        .collect::<Vec<_>>();
    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }

    Err(format!(
        "Unknown model profile `{trimmed}`. Use /model to pick from configured profiles."
    ))
}

fn normalize_model_selector(selector: &str) -> &str {
    let trimmed = selector.trim();
    if !trimmed.starts_with('/') {
        return trimmed;
    }

    let mut parts = trimmed.split_whitespace();
    let Some(command) = parts.next() else {
        return trimmed;
    };
    if command.eq_ignore_ascii_case("/model") {
        return parts.next().unwrap_or("");
    }
    trimmed
}

fn ensure_active_auth_ready(config: &Config) -> Result<(), String> {
    if !config.api.uses_login() {
        return Ok(());
    }
    if !supports_openai_login(&config.api.base_url) {
        return Err(format!(
            "profile `{}` uses `auth = \"login\"`, but base URL `{}` is not an OpenAI login endpoint",
            config.api.profile, config.api.base_url
        ));
    }

    let Some(provider) = login_provider_key_for_base_url(&config.api.base_url) else {
        return Err(format!(
            "profile `{}` uses `auth = \"login\"`, but provider for base URL `{}` is unsupported",
            config.api.profile, config.api.base_url
        ));
    };

    match load_provider_tokens(provider) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err(format!(
            "provider `{}` requires login auth, but no saved login was found. Run `buddy login` (or `/login` inside REPL).",
            provider
        )),
        Err(err) => Err(format!(
            "failed to load login credentials for provider `{}`: {err}",
            provider
        )),
    }
}

async fn run_login_flow(
    renderer: &Renderer,
    config: &Config,
    selector: Option<&str>,
) -> Result<(), String> {
    if config.models.is_empty() {
        return Err(
            "No configured model profiles. Add `[models.<name>]` entries to buddy.toml."
                .to_string(),
        );
    }

    let names = configured_model_profile_names(config);
    let profile_name = if let Some(selector) = selector {
        resolve_model_profile_selector(config, &names, selector)?
    } else {
        config.agent.model.clone()
    };

    let Some(profile) = config.models.get(&profile_name) else {
        return Err(format!("unknown profile `{profile_name}`"));
    };
    if !supports_openai_login(&profile.api_base_url) {
        return Err(format!(
            "profile `{profile_name}` points to `{}`. Login auth currently supports OpenAI endpoints only.",
            profile.api_base_url
        ));
    }

    let Some(provider) = login_provider_key_for_base_url(&profile.api_base_url) else {
        return Err(format!(
            "profile `{profile_name}` points to `{}`. Login auth currently supports OpenAI endpoints only.",
            profile.api_base_url
        ));
    };

    let login = start_openai_device_login()
        .await
        .map_err(|err| format!("failed to start login flow: {err}"))?;

    renderer.section("login");
    renderer.field("profile", &profile_name);
    renderer.field("url", &login.verification_url);
    renderer.field("code", &login.user_code);
    if try_open_browser(&login.verification_url) {
        renderer.field("browser", "opened");
    } else {
        renderer.field("browser", "not available (open URL manually)");
    }
    eprintln!();

    let tokens = {
        let _progress = renderer.progress("waiting for authorization");
        complete_openai_device_login(&login)
            .await
            .map_err(|err| format!("login failed: {err}"))?
    };
    save_provider_tokens(provider, tokens)
        .map_err(|err| format!("failed to save login credentials: {err}"))?;

    renderer.section("login successful");
    renderer.field("profile", &profile_name);
    renderer.field("provider", provider);
    if profile.auth != AuthMode::Login {
        renderer.field(
            "note",
            "this profile currently uses auth=api-key; set auth=login to use saved login tokens",
        );
    }
    eprintln!();

    Ok(())
}

fn render_sessions(renderer: &Renderer, active_session: &str, sessions: &[SessionSummary]) {
    renderer.section("sessions");
    renderer.field("current", active_session);
    if sessions.is_empty() {
        renderer.field("saved", "none");
        eprintln!();
        return;
    }

    for session in sessions {
        let key = if session.id == active_session {
            format!("* {}", session.id)
        } else {
            session.id.clone()
        };
        renderer.field(
            &key,
            &format!(
                "last used {} ago",
                format_elapsed_since_epoch_millis(session.updated_at_millis)
            ),
        );
    }
    eprintln!();
}

fn format_elapsed_since_epoch_millis(ts: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if ts >= now {
        return "0.0s".to_string();
    }
    format_elapsed(Duration::from_millis(now - ts))
}

fn render_status(
    renderer: &Renderer,
    config: &Config,
    agent: Option<&Agent>,
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
        renderer.field("context_limit", "busy (task in progress)");
        renderer.field("messages", "busy (task in progress)");
        renderer.field("session_tokens", "busy (task in progress)");
    }

    eprintln!();
}

fn render_context(renderer: &Renderer, agent: Option<&Agent>, background_tasks: &[BackgroundTask]) {
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
        renderer.field("window_estimate", "busy (task in progress)");
        renderer.field("last_call", "busy (task in progress)");
        renderer.field("session_total", "busy (task in progress)");
        renderer.field("messages", "busy (task in progress)");
    }

    eprintln!();
}

fn enabled_tools(config: &Config, capture_pane_enabled: bool) -> String {
    let tools = enabled_tool_names(config, capture_pane_enabled);
    if tools.is_empty() {
        "none".to_string()
    } else {
        tools.join(", ")
    }
}

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

fn collect_agent_ui_events(
    rx: &mut mpsc::UnboundedReceiver<AgentUiEvent>,
    out: &mut Vec<AgentUiEvent>,
) -> bool {
    let start_len = out.len();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out.len() > start_len
}

fn background_liveness_line(tasks: &[BackgroundTask]) -> Option<String> {
    if tasks.is_empty() {
        return None;
    }

    if tasks.len() == 1 {
        let task = &tasks[0];
        let timeout_suffix = timeout_suffix_for_task(task);
        return Some(match &task.state {
            BackgroundTaskState::Running => format!(
                "task #{} running {}{}",
                task.id,
                format_elapsed_coarse(task.started_at.elapsed()),
                timeout_suffix
            ),
            BackgroundTaskState::WaitingApproval { since, .. } => format!(
                "task #{} waiting approval {}{}",
                task.id,
                format_elapsed_coarse(since.elapsed()),
                timeout_suffix
            ),
            BackgroundTaskState::Cancelling { since } => format!(
                "task #{} cancelling {}{}",
                task.id,
                format_elapsed_coarse(since.elapsed()),
                timeout_suffix
            ),
        });
    }

    let running = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::Running))
        .count();
    let waiting = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::WaitingApproval { .. }))
        .count();
    let cancelling = tasks
        .iter()
        .filter(|task| matches!(task.state, BackgroundTaskState::Cancelling { .. }))
        .count();
    let with_timeout = tasks
        .iter()
        .filter(|task| task.timeout_at.is_some())
        .count();
    let oldest = tasks
        .iter()
        .map(|task| task.started_at.elapsed())
        .max()
        .unwrap_or_default();
    Some(format!(
        "{} tasks: {} running, {} waiting approval, {} cancelling, {} with timeout, oldest {}",
        tasks.len(),
        running,
        waiting,
        cancelling,
        with_timeout,
        format_elapsed_coarse(oldest)
    ))
}

fn timeout_suffix_for_task(task: &BackgroundTask) -> String {
    let Some(timeout_at) = task.timeout_at else {
        return String::new();
    };
    let now = Instant::now();
    if timeout_at <= now {
        " (timeout now)".to_string()
    } else {
        format!(
            " (timeout in {})",
            format_elapsed_coarse(timeout_at.duration_since(now))
        )
    }
}

fn render_agent_ui_events(renderer: &Renderer, events: &mut Vec<AgentUiEvent>) {
    if events.is_empty() {
        return;
    }

    for event in events.drain(..) {
        match event {
            AgentUiEvent::Warning { task_id, message } => {
                renderer.warn(&format!("[task #{task_id}] {message}"));
            }
            AgentUiEvent::TokenUsage {
                task_id,
                prompt_tokens,
                completion_tokens,
                session_total,
            } => {
                renderer.section("task");
                renderer.field(
                    "tokens",
                    &format!(
                        "#{task_id} prompt:{prompt_tokens} completion:{completion_tokens} session:{session_total}"
                    ),
                );
                eprintln!();
            }
            AgentUiEvent::ReasoningTrace {
                task_id,
                field,
                trace,
            } => {
                renderer.reasoning_trace(&format!("task #{task_id} {field}"), &trace);
            }
            AgentUiEvent::ToolCall {
                task_id,
                name,
                args,
            } => {
                let _ = (task_id, name, args);
            }
            AgentUiEvent::ToolResult {
                task_id,
                name,
                args,
                result,
            } => render_tool_result(renderer, task_id, &name, &args, &result),
        }
    }
}

fn render_tool_result(renderer: &Renderer, task_id: u64, name: &str, args: &str, result: &str) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellToolResult {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

fn parse_shell_tool_result(result: &str) -> Option<ShellToolResult> {
    let (exit_line, remainder) = result.split_once("\nstdout:\n")?;
    let exit_code = exit_line
        .trim()
        .strip_prefix("exit code: ")?
        .trim()
        .parse::<i32>()
        .ok()?;
    let (stdout, stderr) = remainder
        .split_once("\nstderr:\n")
        .unwrap_or((remainder, ""));
    Some(ShellToolResult {
        exit_code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    })
}

fn parse_tool_arg(args: &str, key: &str) -> Option<String> {
    let value: Value = serde_json::from_str(args).ok()?;
    value.get(key)?.as_str().map(str::to_string)
}

fn quote_preview(text: &str, max_len: usize) -> String {
    truncate_preview(text, max_len).replace('"', "\\\"")
}

struct BackgroundTask {
    id: u64,
    kind: String,
    details: String,
    started_at: Instant,
    state: BackgroundTaskState,
    timeout_at: Option<Instant>,
    cancel_tx: watch::Sender<bool>,
    handle: JoinHandle<Result<String, buddy::error::AgentError>>,
}

fn has_finished_background_tasks(tasks: &[BackgroundTask]) -> bool {
    tasks.iter().any(|task| task.handle.is_finished())
}

enum BackgroundTaskState {
    Running,
    WaitingApproval { command: String, since: Instant },
    Cancelling { since: Instant },
}

#[derive(Debug, Clone, Copy)]
enum ApprovalPolicy {
    Ask,
    All,
    None,
    Until(Instant),
}

struct PendingApproval {
    task_id: u64,
    command: String,
    request: ShellApprovalRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStartupState {
    ResumedExisting,
    StartedNew,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ResumeRequest {
    SessionId(String),
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalDecision {
    Approve,
    Deny,
}

fn parse_approval_decision(input: &str) -> Option<ApprovalDecision> {
    let normalized = input.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "y" | "yes" => Some(ApprovalDecision::Approve),
        "" | "n" | "no" => Some(ApprovalDecision::Deny),
        _ => None,
    }
}

fn approval_policy_label(policy: ApprovalPolicy) -> String {
    match policy {
        ApprovalPolicy::Ask => "ask".to_string(),
        ApprovalPolicy::All => "all".to_string(),
        ApprovalPolicy::None => "none".to_string(),
        ApprovalPolicy::Until(until) => {
            let now = Instant::now();
            if until <= now {
                "ask".to_string()
            } else {
                format!("auto ({})", format_elapsed(until.duration_since(now)))
            }
        }
    }
}

fn active_approval_decision(policy: &mut ApprovalPolicy) -> Option<ApprovalDecision> {
    match *policy {
        ApprovalPolicy::Ask => None,
        ApprovalPolicy::All => Some(ApprovalDecision::Approve),
        ApprovalPolicy::None => Some(ApprovalDecision::Deny),
        ApprovalPolicy::Until(until) => {
            if until > Instant::now() {
                Some(ApprovalDecision::Approve)
            } else {
                *policy = ApprovalPolicy::Ask;
                None
            }
        }
    }
}

fn update_approval_policy(input: &str, policy: &mut ApprovalPolicy) -> Result<String, String> {
    let normalized = input.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Usage: /approve all|ask|none|<duration>".to_string());
    }

    match normalized.as_str() {
        "ask" => {
            *policy = ApprovalPolicy::Ask;
            Ok("approval policy: ask".to_string())
        }
        "all" => {
            *policy = ApprovalPolicy::All;
            Ok("approval policy: all".to_string())
        }
        "none" => {
            *policy = ApprovalPolicy::None;
            Ok("approval policy: none".to_string())
        }
        _ => {
            let duration = parse_duration_arg(&normalized)
                .ok_or_else(|| "Invalid duration. Examples: 30s, 10m, 1h.".to_string())?;
            *policy = ApprovalPolicy::Until(Instant::now() + duration);
            Ok(format!(
                "approval policy: auto-approve for {}",
                format_elapsed(duration)
            ))
        }
    }
}

fn parse_duration_arg(input: &str) -> Option<Duration> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (digits, unit) = if s.ends_with("ms") {
        (&s[..s.len() - 2], "ms")
    } else if let Some(last) = s.chars().last() {
        if last.is_ascii_alphabetic() {
            (&s[..s.len() - 1], &s[s.len() - 1..])
        } else {
            (s.as_str(), "s")
        }
    } else {
        return None;
    };
    let value = digits.parse::<u64>().ok()?;
    match unit {
        "ms" => Some(Duration::from_millis(value)),
        "s" => Some(Duration::from_secs(value)),
        "m" => value.checked_mul(60).map(Duration::from_secs),
        "h" => value
            .checked_mul(60)
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        "d" => value
            .checked_mul(24)
            .and_then(|v| v.checked_mul(60))
            .and_then(|v| v.checked_mul(60))
            .map(Duration::from_secs),
        _ => None,
    }
}

fn apply_task_timeout_command(
    tasks: &mut [BackgroundTask],
    duration_arg: Option<&str>,
    task_id_arg: Option<&str>,
) -> Result<String, String> {
    let Some(duration_arg) = duration_arg else {
        return Err("Usage: /timeout <duration> [task-id]".to_string());
    };
    let duration = parse_duration_arg(duration_arg)
        .ok_or_else(|| "Invalid duration. Examples: 30s, 10m, 1h.".to_string())?;

    let task_id = if let Some(id_arg) = task_id_arg {
        id_arg.parse::<u64>().map_err(|_| {
            "Task id must be a number. Usage: /timeout <duration> [task-id]".to_string()
        })?
    } else if tasks.is_empty() {
        return Err("No running background tasks.".to_string());
    } else if tasks.len() == 1 {
        tasks[0].id
    } else {
        return Err("Task id required when multiple background tasks are running.".to_string());
    };

    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return Err(format!("No running background task with id #{task_id}."));
    };
    task.timeout_at = Some(Instant::now() + duration);
    Ok(format!(
        "task #{task_id} timeout set to {}",
        format_elapsed(duration)
    ))
}

fn has_elapsed_timeouts(tasks: &[BackgroundTask]) -> bool {
    let now = Instant::now();
    tasks.iter().any(|task| {
        task.timeout_at.is_some_and(|timeout_at| {
            timeout_at <= now && !matches!(task.state, BackgroundTaskState::Cancelling { .. })
        })
    })
}

fn approval_prompt_actor(
    ssh_target: Option<&str>,
    container: Option<&str>,
    tmux_info: Option<&TmuxAttachInfo>,
) -> String {
    let mut actor = if let Some(target) = ssh_target {
        format!("ssh:{target}")
    } else if let Some(container) = container {
        format!("container:{container}")
    } else {
        "local".to_string()
    };

    if let Some(info) = tmux_info {
        actor.push_str(&format!(" (tmux:{})", info.session));
    }
    actor
}

fn render_shell_approval_request(renderer: &Renderer, actor: &str, command: &str) {
    renderer.activity(&format!("shell command on {actor}"));
    renderer.approval_block(&format_approval_command_block(command));
}

fn format_approval_command_block(command: &str) -> String {
    if command.trim().is_empty() {
        return "$".to_string();
    }

    let mut out = String::new();
    for (idx, line) in command.lines().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        if idx == 0 {
            out.push_str("$ ");
        } else {
            out.push_str("  ");
        }
        out.push_str(line);
    }
    out
}

fn session_startup_message(
    state: SessionStartupState,
    session_id: &str,
    context_used: u16,
) -> String {
    match state {
        SessionStartupState::ResumedExisting => {
            format!("using existing session \"{session_id}\" ({context_used}% context used)")
        }
        SessionStartupState::StartedNew => {
            format!("using new session \"{session_id}\" ({context_used}% context used)")
        }
    }
}

fn execution_tmux_attach_command(info: &TmuxAttachInfo) -> String {
    match &info.target {
        TmuxAttachTarget::Local => format!("tmux attach -t {}", info.session),
        TmuxAttachTarget::Ssh { target } => {
            format!("ssh -t {target} tmux attach -t {}", info.session)
        }
        TmuxAttachTarget::Container { engine, container } => {
            format!(
                "{engine} exec -it {container} tmux attach -t {}",
                info.session
            )
        }
    }
}

fn execution_target_label(info: Option<&TmuxAttachInfo>) -> String {
    match info {
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Local,
            ..
        }) => "localhost".to_string(),
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Ssh { target },
            ..
        }) => format!("ssh:{target}"),
        Some(TmuxAttachInfo {
            target: TmuxAttachTarget::Container { container, .. },
            ..
        }) => format!("container:{container}"),
        None => "localhost".to_string(),
    }
}

fn render_startup_banner(color: bool, model: &str, tmux_info: Option<&TmuxAttachInfo>) {
    let target = execution_target_label(tmux_info);
    if color {
        eprintln!(
            "{} {} running on {} with model {}",
            "•".with(Color::DarkGrey),
            "buddy".with(Color::Green).bold(),
            target.as_str().with(Color::White).bold(),
            model.with(Color::Yellow).bold(),
        );
    } else {
        eprintln!("• buddy running on {target} with model {model}");
    }

    if let Some(info) = tmux_info {
        let attach = execution_tmux_attach_command(info);
        if color {
            eprintln!(
                "  attach with: {}",
                attach.as_str().with(Color::White).bold()
            );
        } else {
            eprintln!("  attach with: {attach}");
        }
    }
    eprintln!();
}

fn render_session_startup_line(
    color: bool,
    state: SessionStartupState,
    session_id: &str,
    context_used: u16,
) {
    let message = session_startup_message(state, session_id, context_used);
    if color {
        eprintln!("{} {}", "•".with(Color::DarkGrey), message);
    } else {
        eprintln!("• {message}");
    }
    eprintln!();
}

fn resume_request_from_command(
    command: Option<&cli::Command>,
) -> Result<Option<ResumeRequest>, String> {
    let Some(command) = command else {
        return Ok(None);
    };

    match command {
        cli::Command::Resume { session_id, last } => {
            if *last {
                if session_id.is_some() {
                    return Err(
                        "Use either `buddy resume <session-id>` or `buddy resume --last`."
                            .to_string(),
                    );
                }
                return Ok(Some(ResumeRequest::Last));
            }
            let Some(session_id) = session_id.as_deref().map(str::trim) else {
                return Err("Usage: buddy resume <session-id> | buddy resume --last".to_string());
            };
            if session_id.is_empty() {
                return Err("session id cannot be empty".to_string());
            }
            Ok(Some(ResumeRequest::SessionId(session_id.to_string())))
        }
        _ => Ok(None),
    }
}

fn initialize_active_session(
    renderer: &Renderer,
    session_store: &SessionStore,
    agent: &mut Agent,
    resume_request: Option<ResumeRequest>,
) -> Result<(SessionStartupState, String), String> {
    match resume_request {
        None => {
            let snapshot = agent.snapshot_session();
            let session_id = session_store
                .create_new_session(&snapshot)
                .map_err(|e| format!("failed to create new session: {e}"))?;
            Ok((SessionStartupState::StartedNew, session_id))
        }
        Some(ResumeRequest::Last) => {
            let Some(last_id) = session_store
                .resolve_last()
                .map_err(|e| format!("failed to resolve last session: {e}"))?
            else {
                return Err(
                    "No saved sessions found in this directory. Start `buddy` to create one."
                        .to_string(),
                );
            };
            let snapshot = session_store
                .load(&last_id)
                .map_err(|e| format!("failed to load session {last_id}: {e}"))?;
            agent.restore_session(snapshot.clone());
            if let Err(e) = session_store.save(&last_id, &snapshot) {
                renderer.warn(&format!("failed to refresh session {last_id}: {e}"));
            }
            Ok((SessionStartupState::ResumedExisting, last_id))
        }
        Some(ResumeRequest::SessionId(session_id)) => {
            let snapshot = session_store
                .load(&session_id)
                .map_err(|e| format!("failed to load session {session_id}: {e}"))?;
            agent.restore_session(snapshot.clone());
            if let Err(e) = session_store.save(&session_id, &snapshot) {
                renderer.warn(&format!("failed to refresh session {session_id}: {e}"));
            }
            Ok((SessionStartupState::ResumedExisting, session_id))
        }
    }
}

fn context_used_percent(agent: &Agent) -> Option<u16> {
    let tracker = agent.tracker();
    if tracker.context_limit == 0 {
        return None;
    }
    let estimated = TokenTracker::estimate_messages(agent.messages());
    let percent = ((estimated as f64 / tracker.context_limit as f64) * 100.0).round();
    Some(percent.clamp(0.0, 9_999.0) as u16)
}

fn collect_approval_request(
    renderer: &Renderer,
    tasks: &mut [BackgroundTask],
    approval_rx: &mut Option<mpsc::UnboundedReceiver<ShellApprovalRequest>>,
    approval_policy: &mut ApprovalPolicy,
) -> Option<PendingApproval> {
    let rx = approval_rx.as_mut()?;
    let request = match rx.try_recv() {
        Ok(request) => request,
        Err(mpsc::error::TryRecvError::Empty) => return None,
        Err(mpsc::error::TryRecvError::Disconnected) => return None,
    };

    let Some(task_id) = mark_task_waiting_for_approval(tasks, request.command()) else {
        renderer.warn("Approval request arrived with no running background task; denying it.");
        request.deny();
        return None;
    };

    let pending = PendingApproval {
        task_id,
        command: request.command().to_string(),
        request,
    };

    if let Some(decision) = active_approval_decision(approval_policy) {
        mark_task_running(tasks, task_id);
        match decision {
            ApprovalDecision::Approve => {
                pending.request.approve();
                renderer.section(&format!("task #{task_id} auto-approved"));
                eprintln!();
            }
            ApprovalDecision::Deny => {
                pending.request.deny();
                renderer.section(&format!("task #{task_id} auto-denied"));
                eprintln!();
            }
        }
        None
    } else {
        Some(pending)
    }
}

fn mark_task_waiting_for_approval(tasks: &mut [BackgroundTask], command: &str) -> Option<u64> {
    let task = tasks
        .iter_mut()
        .filter(|task| matches!(task.state, BackgroundTaskState::Running))
        .min_by_key(|task| task.started_at)?;
    task.state = BackgroundTaskState::WaitingApproval {
        command: truncate_preview(command, 96),
        since: Instant::now(),
    };
    Some(task.id)
}

fn mark_task_running(tasks: &mut [BackgroundTask], task_id: u64) {
    if let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) {
        task.state = BackgroundTaskState::Running;
    }
}

fn task_is_waiting_for_approval(tasks: &[BackgroundTask], task_id: u64) -> bool {
    tasks
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| matches!(task.state, BackgroundTaskState::WaitingApproval { .. }))
}

fn deny_pending_approval(tasks: &mut [BackgroundTask], approval: PendingApproval) {
    mark_task_running(tasks, approval.task_id);
    approval.request.deny();
}

fn request_task_cancellation(
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
    task_id: u64,
) -> bool {
    let Some(task) = tasks.iter_mut().find(|task| task.id == task_id) else {
        return false;
    };

    if pending_approval
        .as_ref()
        .is_some_and(|approval| approval.task_id == task_id)
    {
        if let Some(approval) = pending_approval.take() {
            approval.request.deny();
        }
    }

    if !*task.cancel_tx.borrow() {
        let _ = task.cancel_tx.send(true);
    }
    task.state = BackgroundTaskState::Cancelling {
        since: Instant::now(),
    };
    true
}

fn kill_background_task(
    renderer: &Renderer,
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
    task_id: u64,
) {
    if request_task_cancellation(tasks, pending_approval, task_id) {
        renderer.warn(&format!("Cancelling task #{task_id}..."));
    } else {
        renderer.warn(&format!("No running background task with id #{task_id}."));
    }
}

fn enforce_task_timeouts(
    renderer: &Renderer,
    tasks: &mut [BackgroundTask],
    pending_approval: &mut Option<PendingApproval>,
) {
    let now = Instant::now();
    let expired_ids = tasks
        .iter()
        .filter_map(|task| {
            let timeout_at = task.timeout_at?;
            if timeout_at <= now && !matches!(task.state, BackgroundTaskState::Cancelling { .. }) {
                Some(task.id)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    for task_id in expired_ids {
        if request_task_cancellation(tasks, pending_approval, task_id) {
            renderer.warn(&format!("Task #{task_id} hit timeout; cancelling."));
        }
    }
}

async fn drain_finished_tasks(renderer: &Renderer, tasks: &mut Vec<BackgroundTask>) -> bool {
    let mut completed_any = false;
    let mut idx = 0usize;
    while idx < tasks.len() {
        if !tasks[idx].handle.is_finished() {
            idx += 1;
            continue;
        }

        completed_any = true;
        let task = tasks.swap_remove(idx);
        let elapsed = format_elapsed(task.started_at.elapsed());
        match task.handle.await {
            Ok(Ok(response)) => {
                renderer.activity(&format!("prompt #{} processed in {elapsed}", task.id));
                renderer.assistant_message(&response);
            }
            Ok(Err(err)) => {
                renderer.error(&format!(
                    "Background task #{} ({}) failed after {elapsed}: {}",
                    task.id, task.kind, err
                ));
            }
            Err(join_err) => {
                if join_err.is_cancelled() {
                    renderer.warn(&format!(
                        "Background task #{} ({}) cancelled.",
                        task.id, task.kind
                    ));
                } else {
                    renderer.error(&format!(
                        "Background task #{} ({}) terminated unexpectedly: {}",
                        task.id, task.kind, join_err
                    ));
                }
            }
        }
    }

    if tasks.is_empty() {
        Renderer::set_progress_enabled(true);
    }
    completed_any
}

fn render_background_tasks(renderer: &Renderer, tasks: &[BackgroundTask]) {
    renderer.section("background tasks");
    if tasks.is_empty() {
        renderer.field("running", "none");
        eprintln!();
        return;
    }

    for task in tasks {
        let state = match &task.state {
            BackgroundTaskState::Running => {
                format!("running ({})", format_elapsed(task.started_at.elapsed()))
            }
            BackgroundTaskState::WaitingApproval { command, since } => {
                format!(
                    "waiting approval {} for: {}",
                    format_elapsed(since.elapsed()),
                    command
                )
            }
            BackgroundTaskState::Cancelling { since } => {
                format!("cancelling ({})", format_elapsed(since.elapsed()))
            }
        };
        let timeout_note = timeout_suffix_for_task(task);
        renderer.field(
            &format!("#{}", task.id),
            &format!(
                "{} \"{}\" [{}{}]",
                task.kind, task.details, state, timeout_note
            ),
        );
    }
    eprintln!();
}

fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

fn format_elapsed_coarse(elapsed: Duration) -> String {
    if elapsed.as_secs() >= 60 {
        format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
    } else {
        format!("{}s", elapsed.as_secs())
    }
}

fn truncate_preview(text: &str, max_len: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    if flat.len() > max_len {
        format!("{}...", &flat[..max_len])
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buddy::config::ApiProtocol;

    fn dummy_handle() -> JoinHandle<Result<String, buddy::error::AgentError>> {
        tokio::spawn(async { Ok::<String, buddy::error::AgentError>(String::new()) })
    }

    #[test]
    fn ensure_active_auth_ready_skips_api_key_mode() {
        let mut cfg = Config::default();
        cfg.api.auth = AuthMode::ApiKey;
        cfg.api.api_key = "sk-test".to_string();
        assert!(ensure_active_auth_ready(&cfg).is_ok());
    }

    #[test]
    fn ensure_active_auth_ready_rejects_non_openai_login_endpoint() {
        let mut cfg = Config::default();
        cfg.api.auth = AuthMode::Login;
        cfg.api.protocol = ApiProtocol::Responses;
        cfg.api.api_key.clear();
        cfg.api.base_url = "https://openrouter.ai/api/v1".to_string();
        let err = ensure_active_auth_ready(&cfg).unwrap_err();
        assert!(err.contains("not an OpenAI login endpoint"));
    }

    #[test]
    fn parse_approval_decision_supports_yes_no_and_default_deny() {
        assert_eq!(
            parse_approval_decision("y"),
            Some(ApprovalDecision::Approve)
        );
        assert_eq!(
            parse_approval_decision("YES"),
            Some(ApprovalDecision::Approve)
        );
        assert_eq!(parse_approval_decision("n"), Some(ApprovalDecision::Deny));
        assert_eq!(parse_approval_decision(""), Some(ApprovalDecision::Deny));
        assert_eq!(parse_approval_decision("maybe"), None);
    }

    #[test]
    fn approval_prompt_actor_prefers_ssh_then_container_then_local() {
        assert_eq!(
            approval_prompt_actor(Some("dev@host"), Some("box"), None),
            "ssh:dev@host"
        );
        assert_eq!(
            approval_prompt_actor(None, Some("box"), None),
            "container:box"
        );
        assert_eq!(approval_prompt_actor(None, None, None), "local");
    }

    #[test]
    fn approval_prompt_actor_includes_tmux_session_when_available() {
        let info = TmuxAttachInfo {
            session: "buddy-a1b2".to_string(),
            window: "buddy-shared",
            target: TmuxAttachTarget::Local,
        };
        assert_eq!(
            approval_prompt_actor(None, None, Some(&info)),
            "local (tmux:buddy-a1b2)"
        );
    }

    #[test]
    fn approval_command_block_formats_multiline_commands() {
        let block = format_approval_command_block("echo 1\necho 2");
        assert_eq!(block, "$ echo 1\n  echo 2");
    }

    #[test]
    fn session_startup_message_is_clear() {
        assert_eq!(
            session_startup_message(SessionStartupState::ResumedExisting, "abcd-1234", 4),
            "using existing session \"abcd-1234\" (4% context used)"
        );
        assert_eq!(
            session_startup_message(SessionStartupState::StartedNew, "abcd-1234", 0),
            "using new session \"abcd-1234\" (0% context used)"
        );
    }

    #[test]
    fn resume_request_validation_rejects_ambiguous_forms() {
        let err = resume_request_from_command(Some(&cli::Command::Resume {
            session_id: Some("abc".to_string()),
            last: true,
        }))
        .expect_err("must reject");
        assert!(err.contains("either"));
    }

    #[test]
    fn context_used_percent_rounds_and_handles_zero_limit() {
        let mut cfg = Config::default();
        cfg.api.context_limit = Some(100);
        let tools = ToolRegistry::new();
        let mut agent = Agent::new(cfg, tools);
        let mut snapshot = agent.snapshot_session();
        snapshot
            .messages
            .push(buddy::types::Message::user("a".repeat(100)));
        agent.restore_session(snapshot);
        let used = context_used_percent(&agent).expect("has context limit");
        assert!(used > 0);

        let mut cfg_zero = Config::default();
        cfg_zero.api.context_limit = Some(0);
        let agent_zero = Agent::new(cfg_zero, ToolRegistry::new());
        assert_eq!(context_used_percent(&agent_zero), None);
    }

    #[test]
    fn parse_shell_tool_result_extracts_code_stdout_and_stderr() {
        let parsed = parse_shell_tool_result("exit code: 7\nstdout:\na\nb\nstderr:\nwarn")
            .expect("shell output should parse");
        assert_eq!(parsed.exit_code, 7);
        assert_eq!(parsed.stdout, "a\nb");
        assert_eq!(parsed.stderr, "warn");
    }

    #[test]
    fn parse_shell_tool_result_rejects_unexpected_shape() {
        assert!(parse_shell_tool_result("not shell output").is_none());
    }

    #[test]
    fn execution_tmux_attach_command_formats_local_target() {
        let cmd = execution_tmux_attach_command(&TmuxAttachInfo {
            session: "buddy-ef1d".to_string(),
            window: "buddy-shared",
            target: TmuxAttachTarget::Local,
        });
        assert_eq!(cmd, "tmux attach -t buddy-ef1d");
    }

    #[test]
    fn resolve_model_profile_selector_accepts_index_and_name() {
        let mut cfg = Config::default();
        cfg.models.insert(
            "kimi".to_string(),
            buddy::config::ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                model: Some("moonshot-v1".to_string()),
                ..buddy::config::ModelConfig::default()
            },
        );
        let names = configured_model_profile_names(&cfg);

        let by_name = resolve_model_profile_selector(&cfg, &names, "kimi").unwrap();
        assert_eq!(by_name, "kimi");

        let by_index = resolve_model_profile_selector(&cfg, &names, "2").unwrap();
        assert_eq!(by_index, names[1]);
    }

    #[test]
    fn resolve_model_profile_selector_accepts_slash_prefixed_input() {
        let mut cfg = Config::default();
        cfg.models.insert(
            "kimi".to_string(),
            buddy::config::ModelConfig {
                api_base_url: "https://api.moonshot.ai/v1".to_string(),
                model: Some("moonshot-v1".to_string()),
                ..buddy::config::ModelConfig::default()
            },
        );
        let names = configured_model_profile_names(&cfg);

        let by_prefixed_name = resolve_model_profile_selector(&cfg, &names, "/model kimi").unwrap();
        assert_eq!(by_prefixed_name, "kimi");

        let by_prefixed_index = resolve_model_profile_selector(&cfg, &names, "/model 2").unwrap();
        assert_eq!(by_prefixed_index, names[1]);
    }

    #[test]
    fn resolve_model_profile_selector_rejects_unknown() {
        let cfg = Config::default();
        let names = configured_model_profile_names(&cfg);
        let err = resolve_model_profile_selector(&cfg, &names, "missing").unwrap_err();
        assert!(err.contains("Unknown model profile"));
    }

    #[tokio::test]
    async fn background_liveness_line_includes_running_task_state() {
        let task = BackgroundTask {
            id: 3,
            kind: "prompt".into(),
            details: "demo".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            cancel_tx: watch::channel(false).0,
            handle: dummy_handle(),
        };
        let line = background_liveness_line(&[task]).expect("line expected");
        assert!(line.contains("task #3 running"), "line: {line}");
    }

    #[tokio::test]
    async fn collect_approval_request_marks_task_waiting() {
        let (broker, rx) = ShellApprovalBroker::channel();
        let waiter = tokio::spawn(async move { broker.request("ls -la".to_string()).await });
        tokio::task::yield_now().await;

        let mut tasks = vec![BackgroundTask {
            id: 1,
            kind: "prompt".into(),
            details: "list files".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            cancel_tx: watch::channel(false).0,
            handle: dummy_handle(),
        }];
        let mut approval_rx = Some(rx);
        let mut approval_policy = ApprovalPolicy::Ask;
        let renderer = Renderer::new(false);

        let pending = collect_approval_request(
            &renderer,
            &mut tasks,
            &mut approval_rx,
            &mut approval_policy,
        )
        .expect("approval request should be collected");
        assert_eq!(pending.task_id, 1);
        assert_eq!(pending.command, "ls -la");
        assert!(task_is_waiting_for_approval(&tasks, 1));

        pending.request.deny();
        let approved = waiter.await.expect("join should succeed").unwrap();
        assert!(!approved);
    }

    #[tokio::test]
    async fn kill_background_task_denies_pending_approval() {
        let (broker, mut rx) = ShellApprovalBroker::channel();
        let waiter = tokio::spawn(async move { broker.request("echo hi".to_string()).await });
        tokio::task::yield_now().await;
        let request = rx.recv().await.expect("request expected");

        let long_handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<String, buddy::error::AgentError>(String::new())
        });
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut tasks = vec![BackgroundTask {
            id: 7,
            kind: "prompt".into(),
            details: "test".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::WaitingApproval {
                command: "echo hi".into(),
                since: Instant::now(),
            },
            timeout_at: None,
            cancel_tx,
            handle: long_handle,
        }];
        let mut pending = Some(PendingApproval {
            task_id: 7,
            command: "echo hi".into(),
            request,
        });
        let renderer = Renderer::new(false);

        kill_background_task(&renderer, &mut tasks, &mut pending, 7);

        assert_eq!(tasks.len(), 1);
        assert!(matches!(
            tasks[0].state,
            BackgroundTaskState::Cancelling { .. }
        ));
        assert!(*cancel_rx.borrow());
        assert!(pending.is_none());
        let approved = waiter.await.expect("join should succeed").unwrap();
        assert!(!approved);
    }

    #[tokio::test]
    async fn mark_task_waiting_for_approval_skips_cancelling_tasks() {
        let mut tasks = vec![
            BackgroundTask {
                id: 1,
                kind: "prompt".into(),
                details: "old".into(),
                started_at: Instant::now() - Duration::from_secs(2),
                state: BackgroundTaskState::Cancelling {
                    since: Instant::now(),
                },
                timeout_at: None,
                cancel_tx: watch::channel(false).0,
                handle: dummy_handle(),
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "new".into(),
                started_at: Instant::now() - Duration::from_secs(1),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                cancel_tx: watch::channel(false).0,
                handle: dummy_handle(),
            },
        ];
        let picked = mark_task_waiting_for_approval(&mut tasks, "ls").expect("task expected");
        assert_eq!(picked, 2);
    }

    #[test]
    fn parse_duration_arg_supports_common_units() {
        assert_eq!(parse_duration_arg("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration_arg("30"), Some(Duration::from_secs(30)));
        assert_eq!(
            parse_duration_arg("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_duration_arg("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration_arg("bad"), None);
    }

    #[test]
    fn update_approval_policy_parses_modes_and_duration() {
        let mut policy = ApprovalPolicy::Ask;
        assert!(update_approval_policy("all", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::All));
        assert!(update_approval_policy("none", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::None));
        assert!(update_approval_policy("ask", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::Ask));
        assert!(update_approval_policy("10m", &mut policy).is_ok());
        assert!(matches!(policy, ApprovalPolicy::Until(_)));
    }

    #[tokio::test]
    async fn apply_task_timeout_requires_task_id_when_ambiguous() {
        let mut tasks = vec![
            BackgroundTask {
                id: 1,
                kind: "prompt".into(),
                details: "a".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                cancel_tx: watch::channel(false).0,
                handle: dummy_handle(),
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "b".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                cancel_tx: watch::channel(false).0,
                handle: dummy_handle(),
            },
        ];
        let err =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect_err("should fail");
        assert!(err.contains("Task id required"));
    }

    #[tokio::test]
    async fn apply_task_timeout_sets_deadline_for_single_task_without_id() {
        let mut tasks = vec![BackgroundTask {
            id: 9,
            kind: "prompt".into(),
            details: "single".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            cancel_tx: watch::channel(false).0,
            handle: dummy_handle(),
        }];
        let ok =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect("timeout should set");
        assert!(ok.contains("#9"));
        assert!(tasks[0].timeout_at.is_some());
    }
}
