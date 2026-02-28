//! CLI entry point for buddy.

mod app;
mod cli;
mod cli_event_renderer;
mod repl_support;

use app::approval::{
    approval_prompt_actor, deny_pending_approval, render_shell_approval_request,
    send_approval_decision,
};
use app::commands::model::{
    configured_model_profile_names, handle_model_command, resolve_model_profile_selector,
};
use app::commands::session::{
    handle_session_command, initialize_active_session, resume_request_from_command,
};
use app::repl_loop::{
    dispatch_shared_slash_action, SharedSlashDispatchMode, SharedSlashDispatchOutcome,
};
use app::startup::{render_session_startup_line, render_startup_banner};
use app::tasks::{
    background_liveness_line, collect_runtime_events, drain_completed_tasks, enforce_task_timeouts,
    process_runtime_events,
};
use buddy::agent::Agent;
use buddy::auth::{
    complete_openai_device_login, has_legacy_profile_token_records,
    login_provider_key_for_base_url, provider_login_health, reset_provider_tokens,
    save_provider_tokens, start_openai_device_login, supports_openai_login, try_open_browser,
};
use buddy::config::default_history_path;
use buddy::config::ensure_default_global_config;
use buddy::config::initialize_default_global_config;
use buddy::config::load_config_with_diagnostics;
use buddy::config::select_model_profile;
use buddy::config::{AuthMode, Config, GlobalConfigInitResult, ToolsConfig};
use buddy::preflight::validate_active_profile_ready;
use buddy::prompt::{render_system_prompt, ExecutionTarget, SystemPromptParams};
use buddy::render::{set_progress_enabled, RenderSink, Renderer};
#[cfg(test)]
use buddy::runtime::BuddyRuntimeHandle;
use buddy::runtime::{
    spawn_runtime_with_agent, spawn_runtime_with_shared_agent, ModelEvent, PromptMetadata,
    RuntimeCommand, RuntimeEvent, RuntimeEventEnvelope, TaskEvent,
};
use buddy::session::{default_uses_legacy_root, SessionStore};
use buddy::tokens::TokenTracker;
use buddy::tools::capture_pane::CapturePaneTool;
use buddy::tools::execution::ExecutionContext;
use buddy::tools::fetch::FetchTool;
use buddy::tools::files::{ReadFileTool, WriteFileTool};
use buddy::tools::search::WebSearchTool;
use buddy::tools::send_keys::SendKeysTool;
use buddy::tools::shell::{ShellApprovalBroker, ShellTool};
use buddy::tools::time::TimeTool;
use buddy::tools::ToolRegistry;
use buddy::tui as repl;
use clap::Parser;
#[cfg(test)]
use repl_support::BackgroundTaskState;
#[cfg(test)]
use repl_support::{
    apply_task_timeout_command, mark_task_waiting_for_approval, parse_duration_arg,
    parse_shell_tool_result, tool_result_display_text, update_approval_policy, ApprovalDecision,
};
use repl_support::{
    approval_policy_label, has_elapsed_timeouts, mark_task_running, parse_approval_decision,
    task_is_waiting_for_approval, ApprovalPolicy, BackgroundTask, CompletedBackgroundTask,
    PendingApproval, RuntimeContextState,
};
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::sync::mpsc;
use tokio::sync::Mutex;

const BACKGROUND_TASK_WARNING: &str =
    "Background tasks are in progress. Allowed commands now: /ps, /kill <id>, /timeout <dur> [id], /approve <mode>, /status, /context.";

fn enforce_exec_shell_guardrails(
    is_exec_command: bool,
    dangerously_auto_approve: bool,
    tools: &mut ToolsConfig,
) -> Result<Option<String>, String> {
    if !is_exec_command || !tools.shell_enabled || !tools.shell_confirm {
        return Ok(None);
    }
    if dangerously_auto_approve {
        tools.shell_confirm = false;
        return Ok(Some(
            "Dangerous mode enabled: auto-approving run_shell commands for this exec invocation."
                .to_string(),
        ));
    }
    Err(
        "buddy exec is non-interactive and fails closed when tools.shell_confirm=true. Run interactive buddy, set tools.shell_confirm=false, or pass --dangerously-auto-approve."
            .to_string(),
    )
}

#[tokio::main]
async fn main() {
    let args = cli::Args::parse();
    let renderer = Renderer::new(!args.no_color);

    if let Some(cli::Command::Init { force }) = args.command.as_ref() {
        if let Err(msg) = run_init_flow(&renderer, *force) {
            renderer.error(&msg);
            std::process::exit(1);
        }
        return;
    }

    if let Err(e) = ensure_default_global_config() {
        eprintln!("warning: failed to initialize ~/.config/buddy/buddy.toml: {e}");
    }

    // Load config + compatibility diagnostics.
    let loaded_config = match load_config_with_diagnostics(args.config.as_deref()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let mut config = loaded_config.config;
    let mut compatibility_warnings = loaded_config.diagnostics.deprecations;

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
    for warning in compatibility_warnings.drain(..) {
        renderer.warn(&warning);
    }
    match has_legacy_profile_token_records() {
        Ok(true) => renderer.warn(
            "Auth store uses deprecated profile-scoped login records; run `buddy login` to migrate to provider-scoped records before v0.4.",
        ),
        Ok(false) => {}
        Err(err) => renderer.warn(&format!(
            "failed to inspect auth store for legacy credentials: {err}"
        )),
    }

    let resume_request = match resume_request_from_command(args.command.as_ref()) {
        Ok(request) => request,
        Err(msg) => {
            renderer.error(&msg);
            std::process::exit(1);
        }
    };

    if let Some(cli::Command::Login {
        model,
        reset,
        check,
    }) = args.command.as_ref()
    {
        let selector = model.as_deref();
        if let Err(msg) = run_login_flow(&renderer, &config, selector, *reset, *check).await {
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

    if let Err(msg) = validate_active_profile_ready(&config) {
        renderer.error(&msg);
        std::process::exit(1);
    }

    let is_exec_command = matches!(args.command.as_ref(), Some(cli::Command::Exec { .. }));
    match enforce_exec_shell_guardrails(
        is_exec_command,
        args.dangerously_auto_approve,
        &mut config.tools,
    ) {
        Ok(Some(warning)) => renderer.warn(&warning),
        Ok(None) => {}
        Err(msg) => {
            renderer.error(&msg);
            std::process::exit(1);
        }
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
        match ExecutionContext::container_tmux(
            container.clone(),
            requested_tmux_session,
            &config.agent.name,
        )
        .await
        {
            Ok(ctx) => ctx,
            Err(e) => {
                renderer.error(&format!("failed to initialize container execution: {e}"));
                std::process::exit(1);
            }
        }
    } else if let Some(target) = &args.ssh {
        match ExecutionContext::ssh(target.clone(), requested_tmux_session, &config.agent.name)
            .await
        {
            Ok(ctx) => ctx,
            Err(e) => {
                renderer.error(&format!("failed to initialize ssh execution: {e}"));
                std::process::exit(1);
            }
        }
    } else {
        match ExecutionContext::local_tmux(requested_tmux_session, &config.agent.name).await {
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
    let needs_approval_broker = interactive_mode
        && ((config.tools.shell_enabled && config.tools.shell_confirm)
            || (config.tools.fetch_enabled && config.tools.fetch_confirm));
    let (shell_approval_broker, mut shell_approval_rx) = if needs_approval_broker {
        let (broker, rx) = ShellApprovalBroker::channel();
        (Some(broker), Some(rx))
    } else {
        (None, None)
    };

    if config.tools.shell_enabled {
        tools.register(ShellTool {
            confirm: config.tools.shell_confirm,
            denylist: config.tools.shell_denylist.clone(),
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
        tools.register(FetchTool::new(
            Duration::from_secs(config.network.fetch_timeout_secs),
            config.tools.fetch_confirm,
            config.tools.fetch_allowed_domains.clone(),
            config.tools.fetch_blocked_domains.clone(),
            shell_approval_broker.clone(),
        ));
    }
    if config.tools.files_enabled {
        tools.register(ReadFileTool {
            execution: execution.clone(),
        });
        tools.register(WriteFileTool {
            execution: execution.clone(),
            allowed_paths: config.tools.files_allowed_paths.clone(),
        });
    }
    if config.tools.search_enabled {
        tools.register(WebSearchTool::new(Duration::from_secs(
            config.network.fetch_timeout_secs,
        )));
    }
    tools.register(TimeTool);

    // Create agent.
    let mut agent = Agent::new(config.clone(), tools);

    if let Some(cli::Command::Exec { prompt }) = args.command.as_ref() {
        // One-shot mode: run through the runtime actor command/event interface.
        let (runtime, mut events) =
            spawn_runtime_with_agent(agent, config.clone(), None, None, None);
        if let Err(e) = runtime
            .send(RuntimeCommand::SubmitPrompt {
                prompt: prompt.clone(),
                metadata: PromptMetadata {
                    source: Some("cli-exec".to_string()),
                    correlation_id: None,
                },
            })
            .await
        {
            renderer.error(&format!("failed to submit prompt: {e}"));
            std::process::exit(1);
        }

        let mut final_response: Option<String> = None;
        let mut failure_message: Option<String> = None;
        while let Some(envelope) = events.recv().await {
            match envelope.event {
                RuntimeEvent::Model(ModelEvent::MessageFinal { content, .. }) => {
                    final_response = Some(content);
                }
                RuntimeEvent::Task(TaskEvent::Failed { message, .. }) => {
                    failure_message = Some(message);
                }
                RuntimeEvent::Task(TaskEvent::Completed { .. }) => {
                    break;
                }
                _ => {}
            }
        }
        let _ = runtime.send(RuntimeCommand::Shutdown).await;

        if let Some(message) = failure_message {
            renderer.error(&message);
            std::process::exit(1);
        }
        if let Some(response) = final_response {
            renderer.assistant_message(&response);
        } else {
            renderer.error("runtime finished without a final assistant message");
            std::process::exit(1);
        }
    } else {
        // Interactive REPL.
        if default_uses_legacy_root() {
            renderer.warn(
                "Using deprecated `.agentx/` session root; migrate to `.buddyx/` before legacy support is removed after v0.4.",
            );
        }
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
        let mut pending_runtime_events: Vec<RuntimeEventEnvelope> = Vec::new();
        let mut runtime_context =
            RuntimeContextState::new(config.api.context_limit.map(|limit| limit as u64));
        let mut last_prompt_context_used_percent: Option<u16> = None;

        loop {
            enforce_task_timeouts(
                &renderer,
                &runtime,
                &mut background_tasks,
                &mut pending_approval,
            )
            .await;
            collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
            process_runtime_events(
                &renderer,
                &mut pending_runtime_events,
                &mut background_tasks,
                &mut completed_tasks,
                &mut pending_approval,
                &mut config,
                &mut active_session,
                &mut runtime_context,
            );
            let _ = drain_completed_tasks(&renderer, &mut completed_tasks);
            if background_tasks.is_empty() {
                set_progress_enabled(true);
            }

            if let Some(approval) = pending_approval.take() {
                let approval_actor = approval_prompt_actor(
                    args.ssh.as_deref(),
                    args.container.as_deref(),
                    execution.tmux_attach_info().as_ref(),
                );
                render_shell_approval_request(
                    config.display.color,
                    &renderer,
                    &approval_actor,
                    &approval.command,
                    approval.risk.as_deref(),
                    approval.why.as_deref(),
                );
                let approval_prompt = repl::ApprovalPrompt {
                    actor: &approval_actor,
                    command: &approval.command,
                    privileged: approval.privesc.unwrap_or(false),
                    mutation: approval.mutation.unwrap_or(false),
                };

                let approval_input = match repl::read_repl_line_with_interrupt(
                    config.display.color,
                    &mut repl_state,
                    args.ssh.as_deref(),
                    None,
                    repl::PromptMode::Approval,
                    Some(&approval_prompt),
                    || repl::ReadPoll {
                        interrupt: has_elapsed_timeouts(&background_tasks),
                        status_line: None,
                    },
                ) {
                    Ok(repl::ReadOutcome::Line(line)) => line,
                    Ok(repl::ReadOutcome::Eof) => {
                        deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                        continue;
                    }
                    Ok(repl::ReadOutcome::Cancelled) => {
                        deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                        break;
                    }
                    Ok(repl::ReadOutcome::Interrupted) => {
                        pending_approval = Some(approval);
                        continue;
                    }
                    Err(e) => {
                        renderer.error(&format!("failed to read approval input: {e}"));
                        deny_pending_approval(&runtime, &mut background_tasks, approval).await;
                        continue;
                    }
                };

                let approval_input = approval_input.trim();
                eprintln!();
                if let Some(decision) = parse_approval_decision(approval_input) {
                    let task_id = approval.task_id;
                    if let Err(err) = send_approval_decision(&runtime, &approval, decision).await {
                        renderer.warn(&err);
                    } else {
                        mark_task_running(&mut background_tasks, task_id);
                    }
                    continue;
                }

                if let Some(action) = repl::parse_slash_command(approval_input) {
                    match &action {
                        repl::SlashCommandAction::Status => {
                            let guard = agent.try_lock().ok();
                            render_status(
                                &renderer,
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
                        repl::SlashCommandAction::Context => {
                            let guard = agent.try_lock().ok();
                            render_context(
                                &renderer,
                                guard.as_deref(),
                                runtime_context,
                                &background_tasks,
                            );
                            pending_approval = Some(approval);
                            continue;
                        }
                        _ => {}
                    }
                    let dispatch_outcome = dispatch_shared_slash_action(
                        &renderer,
                        &action,
                        &runtime,
                        &mut background_tasks,
                        &mut pending_approval,
                        Some(&approval),
                        &mut approval_policy,
                        SharedSlashDispatchMode::Approval {
                            task_id: approval.task_id,
                        },
                    )
                    .await;
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
            let input = match repl::read_repl_line_with_interrupt(
                config.display.color,
                &mut repl_state,
                args.ssh.as_deref(),
                last_prompt_context_used_percent,
                repl::PromptMode::Normal,
                None,
                || {
                    // If runtime events arrive while the input editor is visible, interrupt the
                    // editor immediately. Rendering those events while raw-mode input is active
                    // causes overlapping lines and cursor drift.
                    let has_new_runtime_events =
                        collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
                    repl::ReadPoll {
                        interrupt: has_new_runtime_events
                            || has_elapsed_timeouts(&background_tasks),
                        status_line: background_liveness_line(&background_tasks),
                    }
                },
            ) {
                Ok(repl::ReadOutcome::Line(line)) => line,
                Ok(repl::ReadOutcome::Eof) => break,
                Ok(repl::ReadOutcome::Cancelled) => break,
                Ok(repl::ReadOutcome::Interrupted) => continue,
                Err(e) => {
                    renderer.error(&format!("failed to read input: {e}"));
                    break;
                }
            };

            let input = input.trim_end();
            if input.trim().is_empty() {
                continue;
            }

            collect_runtime_events(&mut runtime_events, &mut pending_runtime_events);
            process_runtime_events(
                &renderer,
                &mut pending_runtime_events,
                &mut background_tasks,
                &mut completed_tasks,
                &mut pending_approval,
                &mut config,
                &mut active_session,
                &mut runtime_context,
            );
            let _ = drain_completed_tasks(&renderer, &mut completed_tasks);

            repl_state.push_history(input);
            let has_background_tasks = !background_tasks.is_empty();

            if let Some(action) = repl::parse_slash_command(input) {
                match &action {
                    repl::SlashCommandAction::Status => {
                        let guard = agent.try_lock().ok();
                        render_status(
                            &renderer,
                            &config,
                            guard.as_deref(),
                            runtime_context,
                            &background_tasks,
                            approval_policy,
                            capture_pane_enabled,
                        );
                        continue;
                    }
                    repl::SlashCommandAction::Context => {
                        let guard = agent.try_lock().ok();
                        render_context(
                            &renderer,
                            guard.as_deref(),
                            runtime_context,
                            &background_tasks,
                        );
                        continue;
                    }
                    _ => {}
                }

                let dispatch_outcome = dispatch_shared_slash_action(
                    &renderer,
                    &action,
                    &runtime,
                    &mut background_tasks,
                    &mut pending_approval,
                    None,
                    &mut approval_policy,
                    SharedSlashDispatchMode::Repl,
                )
                .await;
                if !matches!(dispatch_outcome, SharedSlashDispatchOutcome::Unhandled) {
                    continue;
                }

                match action {
                    repl::SlashCommandAction::Quit => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            break;
                        }
                    }
                    repl::SlashCommandAction::Session { verb, name } => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            handle_session_command(
                                &renderer,
                                &session_store,
                                &runtime,
                                &mut active_session,
                                verb.as_deref(),
                                name.as_deref(),
                            )
                            .await;
                        }
                    }
                    repl::SlashCommandAction::Compact => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else if let Err(e) = runtime.send(RuntimeCommand::SessionCompact).await {
                            renderer
                                .warn(&format!("failed to submit session compact command: {e}"));
                        }
                    }
                    repl::SlashCommandAction::Model(selector) => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else {
                            handle_model_command(
                                &renderer,
                                &mut config,
                                &runtime,
                                selector.as_deref(),
                            )
                            .await;
                        }
                    }
                    repl::SlashCommandAction::Login(selector) => {
                        if has_background_tasks {
                            renderer.warn(BACKGROUND_TASK_WARNING);
                        } else if let Err(msg) =
                            run_login_flow(&renderer, &config, selector.as_deref(), false, false)
                                .await
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
                    repl::SlashCommandAction::Ps
                    | repl::SlashCommandAction::Kill(_)
                    | repl::SlashCommandAction::Timeout { .. }
                    | repl::SlashCommandAction::Approve(_) => {}
                    repl::SlashCommandAction::Status | repl::SlashCommandAction::Context => {}
                }
                continue;
            }

            if has_background_tasks {
                renderer.warn(BACKGROUND_TASK_WARNING);
                continue;
            }

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
    }
}

fn render_help(renderer: &dyn RenderSink) {
    renderer.section("slash commands");
    for cmd in &repl::SLASH_COMMANDS {
        renderer.field(cmd.name, cmd.description);
    }
    eprintln!();
}

fn run_init_flow(renderer: &dyn RenderSink, force: bool) -> Result<(), String> {
    match initialize_default_global_config(force)
        .map_err(|e| format!("failed to initialize ~/.config/buddy: {e}"))?
    {
        GlobalConfigInitResult::Created { path } => {
            renderer.section("initialized buddy config");
            renderer.field("path", &path.display().to_string());
            eprintln!();
            Ok(())
        }
        GlobalConfigInitResult::Overwritten { path, backup_path } => {
            renderer.section("reinitialized buddy config");
            renderer.field("path", &path.display().to_string());
            renderer.field("backup", &backup_path.display().to_string());
            eprintln!();
            Ok(())
        }
        GlobalConfigInitResult::AlreadyInitialized { path } => Err(format!(
            "buddy is already initialized at {}. Use `buddy init --force` to overwrite.",
            path.display()
        )),
    }
}

async fn run_login_flow(
    renderer: &dyn RenderSink,
    config: &Config,
    selector: Option<&str>,
    reset: bool,
    check: bool,
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

    let health = provider_login_health(provider)
        .map_err(|err| format!("failed to check existing login health: {err}"))?;
    renderer.section("login health");
    renderer.field("provider", provider);
    renderer.field(
        "saved_credentials",
        if health.has_tokens { "yes" } else { "no" },
    );
    if let Some(expires_at_unix) = health.expires_at_unix {
        renderer.field("expires_at_unix", &expires_at_unix.to_string());
        renderer.field(
            "expiring_soon",
            if health.expiring_soon { "yes" } else { "no" },
        );
    }
    eprintln!();

    if check {
        return Ok(());
    }

    if reset {
        let removed = reset_provider_tokens(provider)
            .map_err(|err| format!("failed to reset saved login credentials: {err}"))?;
        if removed {
            renderer.section("login reset");
            renderer.field("provider", provider);
            renderer.field("status", "removed saved credentials");
            eprintln!();
        } else {
            renderer.section("login reset");
            renderer.field("provider", provider);
            renderer.field("status", "no saved credentials found");
            eprintln!();
        }
    }

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

fn context_used_percent(agent: &Agent) -> Option<u16> {
    let tracker = agent.tracker();
    if tracker.context_limit == 0 {
        return None;
    }
    let estimated = TokenTracker::estimate_messages(agent.messages());
    let percent = (estimated as f64 / tracker.context_limit as f64) * 100.0;
    Some(display_context_percent(percent))
}

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
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct MockRenderer {
        entries: Arc<StdMutex<Vec<(String, String)>>>,
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

        fn progress(&self, label: &str) -> buddy::render::ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn progress_with_metrics(
            &self,
            label: &str,
            _metrics: buddy::render::ProgressMetrics,
        ) -> buddy::render::ProgressHandle {
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
    fn exec_shell_guardrails_fail_closed_without_override() {
        let mut tools = ToolsConfig::default();
        tools.shell_enabled = true;
        tools.shell_confirm = true;
        let err = enforce_exec_shell_guardrails(true, false, &mut tools).unwrap_err();
        assert!(
            err.contains("--dangerously-auto-approve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exec_shell_guardrails_auto_approve_disables_shell_confirm() {
        let mut tools = ToolsConfig::default();
        tools.shell_enabled = true;
        tools.shell_confirm = true;
        let warning = enforce_exec_shell_guardrails(true, true, &mut tools)
            .expect("guardrail should allow override")
            .expect("warning expected");
        assert!(warning.contains("Dangerous mode"));
        assert!(!tools.shell_confirm);
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
    fn parse_shell_tool_result_extracts_from_enveloped_json() {
        let result = serde_json::json!({
            "harness_timestamp": { "source": "harness", "unix_millis": 123 },
            "result": {
                "exit_code": 9,
                "stdout": "a",
                "stderr": "b"
            }
        })
        .to_string();
        let parsed = parse_shell_tool_result(&result).expect("parse shell payload");
        assert_eq!(parsed.exit_code, 9);
        assert_eq!(parsed.stdout, "a");
        assert_eq!(parsed.stderr, "b");
    }

    #[test]
    fn tool_result_display_text_unwraps_envelope_strings() {
        let result = serde_json::json!({
            "harness_timestamp": { "source": "harness", "unix_millis": 123 },
            "result": "hello"
        })
        .to_string();
        assert_eq!(tool_result_display_text(&result), "hello");
    }

    #[test]
    fn background_liveness_line_includes_running_task_state() {
        let task = BackgroundTask {
            id: 3,
            kind: "prompt".into(),
            details: "demo".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            final_response: None,
        };
        let line = background_liveness_line(&[task]).expect("line expected");
        assert!(line.contains("task #3 running"), "line: {line}");
    }

    #[test]
    fn mark_task_waiting_for_approval_marks_selected_task() {
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
                final_response: None,
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "new".into(),
                started_at: Instant::now() - Duration::from_secs(1),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
        ];
        assert!(mark_task_waiting_for_approval(
            &mut tasks,
            2,
            "ls",
            Some("low".to_string()),
            Some(false),
            Some(false),
            Some("inspect files".to_string()),
            "appr-2"
        ));
        assert!(task_is_waiting_for_approval(&tasks, 2));
        assert!(!task_is_waiting_for_approval(&tasks, 1));
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

    #[test]
    fn apply_task_timeout_requires_task_id_when_ambiguous() {
        let mut tasks = vec![
            BackgroundTask {
                id: 1,
                kind: "prompt".into(),
                details: "a".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
            BackgroundTask {
                id: 2,
                kind: "prompt".into(),
                details: "b".into(),
                started_at: Instant::now(),
                state: BackgroundTaskState::Running,
                timeout_at: None,
                final_response: None,
            },
        ];
        let err =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect_err("should fail");
        assert!(err.contains("Task id required"));
    }

    #[test]
    fn apply_task_timeout_sets_deadline_for_single_task_without_id() {
        let mut tasks = vec![BackgroundTask {
            id: 9,
            kind: "prompt".into(),
            details: "single".into(),
            started_at: Instant::now(),
            state: BackgroundTaskState::Running,
            timeout_at: None,
            final_response: None,
        }];
        let ok =
            apply_task_timeout_command(&mut tasks, Some("10m"), None).expect("timeout should set");
        assert!(ok.contains("#9"));
        assert!(tasks[0].timeout_at.is_some());
    }

    #[tokio::test]
    async fn handle_session_command_new_submits_runtime_command() {
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = Renderer::new(false);
        let mut active = "abcd-1234".to_string();

        handle_session_command(&renderer, &store, &runtime, &mut active, Some("new"), None).await;

        let command = rx.recv().await.expect("command expected");
        assert!(matches!(command, RuntimeCommand::SessionNew));
    }

    #[tokio::test]
    async fn handle_session_command_resume_without_id_warns() {
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-missing-id-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut active = "abcd-1234".to_string();

        handle_session_command(
            &renderer,
            &store,
            &runtime,
            &mut active,
            Some("resume"),
            None,
        )
        .await;

        assert!(
            renderer.saw("warn", "Usage: /session resume <session-id|last>"),
            "expected usage warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[tokio::test]
    async fn handle_session_command_resume_last_without_sessions_warns() {
        let temp = std::env::temp_dir().join(format!(
            "buddy-main-session-test-last-empty-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let store = SessionStore::open(&temp).expect("open session store");
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut active = "abcd-1234".to_string();

        handle_session_command(
            &renderer,
            &store,
            &runtime,
            &mut active,
            Some("resume"),
            Some("last"),
        )
        .await;

        assert!(
            renderer.saw("warn", "No saved sessions found."),
            "expected empty-session warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[tokio::test]
    async fn handle_model_command_submits_switch_command() {
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = Renderer::new(false);
        let mut config = Config::default();

        handle_model_command(
            &renderer,
            &mut config,
            &runtime,
            Some("openrouter-deepseek"),
        )
        .await;

        let command = rx.recv().await.expect("command expected");
        match command {
            RuntimeCommand::SwitchModel { profile } => {
                assert_eq!(profile, "openrouter-deepseek");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_model_command_warns_on_unknown_selector() {
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut config = Config::default();

        handle_model_command(&renderer, &mut config, &runtime, Some("missing-profile")).await;

        assert!(
            renderer.saw("warn", "Unknown model profile"),
            "expected unknown-profile warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[test]
    fn process_runtime_events_routes_warning_to_injected_renderer() {
        let renderer = MockRenderer::default();
        let mut events = vec![RuntimeEventEnvelope {
            seq: 1,
            ts_unix_ms: 1,
            event: RuntimeEvent::Warning(buddy::runtime::WarningEvent {
                task: None,
                message: "demo warning".to_string(),
            }),
        }];
        let mut background_tasks = Vec::new();
        let mut completed_tasks = Vec::new();
        let mut pending_approval = None;
        let mut config = Config::default();
        let mut active_session = "session-x".to_string();
        let mut runtime_context = RuntimeContextState::new(None);

        process_runtime_events(
            &renderer,
            &mut events,
            &mut background_tasks,
            &mut completed_tasks,
            &mut pending_approval,
            &mut config,
            &mut active_session,
            &mut runtime_context,
        );

        assert!(
            renderer.saw("warn", "demo warning"),
            "runtime warning should flow through render trait"
        );
    }
}
