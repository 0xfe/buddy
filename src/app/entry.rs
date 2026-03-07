//! Application entry orchestration for the buddy CLI.

use crate::app::commands::auth::resolve_auth_provider_selector;
#[cfg(test)]
use crate::app::commands::model::handle_model_command;
#[cfg(test)]
use crate::app::commands::session::handle_session_command;
use crate::app::commands::session::resume_request_from_command;
use crate::app::init_flow::{maybe_run_auto_init, run_init_flow, InitInvocation};
#[cfg(test)]
use crate::app::tasks::background_liveness_line;
#[cfg(test)]
use crate::app::tasks::{process_runtime_events, ProcessRuntimeEventsContext};
use crate::app::trace::resolve_trace_path;
use crate::app::trace_cli::run_trace_command;
use crate::cli;
use buddy::agent::Agent;
use buddy::api::default_builtin_tool_names;
use buddy::auth::{
    complete_openai_device_login, has_legacy_profile_token_records, provider_login_health,
    reset_provider_tokens, save_provider_tokens, start_openai_device_login, try_open_browser,
};
use buddy::config::load_config_with_diagnostics;
use buddy::config::select_model_profile;
#[cfg(test)]
use buddy::config::ModelProvider;
use buddy::config::{AuthMode, Config, ToolsConfig};
use buddy::preflight::validate_active_profile_ready;
use buddy::prompt::{render_system_prompt, ExecutionTarget, SystemPromptParams};
#[cfg(test)]
use buddy::repl::{
    apply_task_timeout_command, mark_task_waiting_for_approval, parse_duration_arg,
    parse_shell_tool_result, tool_result_display_text, update_approval_policy, ApprovalDecision,
};
#[cfg(test)]
use buddy::repl::{
    parse_approval_decision, task_is_waiting_for_approval, ApprovalPolicy, BackgroundTask,
    BackgroundTaskState, RuntimeContextState,
};
#[cfg(test)]
use buddy::runtime::BuddyRuntimeHandle;
#[cfg(test)]
use buddy::runtime::RuntimeCommand;
#[cfg(test)]
use buddy::runtime::{RuntimeEvent, RuntimeEventEnvelope};
#[cfg(test)]
use buddy::session::SessionStore;
use buddy::tools::capture_pane::CapturePaneTool;
use buddy::tools::execution::ExecutionContext;
use buddy::tools::fetch::FetchTool;
use buddy::tools::files::{ReadFileTool, WriteFileTool};
use buddy::tools::search::WebSearchTool;
use buddy::tools::send_keys::SendKeysTool;
use buddy::tools::shell::{ShellApprovalBroker, ShellTool};
use buddy::tools::time::TimeTool;
use buddy::tools::tmux_manage::{
    TmuxCreatePaneTool, TmuxCreateSessionTool, TmuxKillPaneTool, TmuxKillSessionTool,
    TmuxToolShared,
};
use buddy::tools::ToolRegistry;
use buddy::ui::render::{RenderSink, Renderer};
use buddy::ui::theme as ui_theme;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
#[cfg(test)]
use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::sync::mpsc;

/// Enforce non-interactive `exec` safety defaults for shell tool confirmations.
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

/// Top-level CLI entrypoint that dispatches init/login/exec/repl flows.
pub(crate) async fn run(args: crate::cli::Args) -> i32 {
    // Entrypoint walkthrough:
    // 1) handle init/login early-return commands,
    // 2) load + validate config/runtime setup,
    // 3) dispatch into one-shot exec mode or interactive REPL mode.
    if args.version {
        println!("{}", buddy::build_info::cli_version_text());
        return 0;
    }

    let bootstrap_renderer = Renderer::new(!args.no_color);
    if let Some(cli::Command::Init { force }) = args.command.as_ref() {
        if let Err(msg) = run_init_flow(
            &bootstrap_renderer,
            InitInvocation::Manual { force: *force },
        )
        .await
        {
            bootstrap_renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    if let Some(cli::Command::Trace { command }) = args.command.as_ref() {
        if let Err(msg) = run_trace_command(&bootstrap_renderer, command) {
            bootstrap_renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    if let Some(cli::Command::Traceui { file, stream }) = args.command.as_ref() {
        if let Err(msg) = buddy::traceui::run(buddy::traceui::TraceUiOptions {
            file: file.into(),
            stream: *stream,
        }) {
            bootstrap_renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    if let Err(msg) = maybe_run_auto_init(&bootstrap_renderer, &args).await {
        bootstrap_renderer.error(&msg);
        return 1;
    }

    let loaded = match load_config_state(&args) {
        Ok(state) => state,
        Err(msg) => {
            bootstrap_renderer.error(&msg);
            return 1;
        }
    };
    if let Err(msg) = initialize_ui_theme(&loaded.config) {
        bootstrap_renderer.warn(&msg);
    }
    let renderer = Renderer::new(loaded.config.display.color);
    for warning in &loaded.warnings {
        renderer.warn(warning);
    }

    if let Some(cli::Command::Login {
        provider,
        reset,
        check,
    }) = args.command.as_ref()
    {
        if let Err(msg) = run_login_flow(
            &renderer,
            &loaded.config,
            provider.as_deref(),
            *reset,
            *check,
        )
        .await
        {
            renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    if let Some(cli::Command::Logout { provider }) = args.command.as_ref() {
        if let Err(msg) = run_logout_flow(&renderer, &loaded.config, provider.as_deref()) {
            renderer.error(&msg);
            return 1;
        }
        return 0;
    }

    let runtime_setup = match prepare_runtime_setup(&args, &renderer, loaded).await {
        Ok(setup) => setup,
        Err(msg) => {
            renderer.error(&msg);
            return 1;
        }
    };
    let trace_path = resolve_trace_path(&args);

    if let Some(cli::Command::Exec { prompt }) = args.command.as_ref() {
        return crate::app::exec_mode::run_exec_mode(
            &renderer,
            runtime_setup.agent,
            runtime_setup.config,
            prompt.clone(),
            trace_path,
        )
        .await;
    }

    crate::app::repl_mode::run_repl_mode(crate::app::repl_mode::ReplModeInputs {
        renderer: &renderer,
        cli_args: &args,
        config: runtime_setup.config,
        execution: runtime_setup.execution,
        capture_pane_enabled: runtime_setup.capture_pane_enabled,
        agent: runtime_setup.agent,
        resume_request: runtime_setup.resume_request,
        shell_approval_rx: runtime_setup.shell_approval_rx,
        trace_path,
    })
    .await
}

/// Initialize global UI theme state from runtime config.
fn initialize_ui_theme(config: &Config) -> Result<(), String> {
    let custom = config
        .themes
        .iter()
        .map(|(name, table)| (name.clone(), table.values.clone()))
        .collect();
    ui_theme::initialize(&config.display.theme, &custom).map_err(|err| {
        format!(
            "failed to initialize theme `{}`: {err}",
            config.display.theme
        )
    })
}

/// Loaded config plus derived warnings/startup resume request.
struct LoadedConfigState {
    /// Effective runtime configuration after CLI overrides.
    config: Config,
    /// User-facing warnings collected during config/auth diagnostics.
    warnings: Vec<String>,
    /// Optional session resume request parsed from CLI command.
    resume_request: Option<buddy::repl::ResumeRequest>,
}

/// Runtime-ready wiring produced before entering exec/repl flows.
struct RuntimeSetup {
    /// Effective runtime configuration.
    config: Config,
    /// Execution backend context (local/ssh/container/tmux).
    execution: ExecutionContext,
    /// Whether tmux capture/send tools can be exposed.
    capture_pane_enabled: bool,
    /// Agent instance bound to the configured tool registry.
    agent: Agent,
    /// Optional startup session resume request.
    resume_request: Option<buddy::repl::ResumeRequest>,
    /// Shell approval request stream when interactive confirmations are enabled.
    shell_approval_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<buddy::tools::shell::ShellApprovalRequest>>,
}

/// Tool registry plus optional shell-approval channel wiring.
struct ToolSetup {
    /// Registered tool implementations for the agent.
    tools: ToolRegistry,
    /// Shell approval request receiver, if confirmations are enabled.
    shell_approval_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<buddy::tools::shell::ShellApprovalRequest>>,
}

/// Load config, apply CLI overrides, and collect startup diagnostics/warnings.
fn load_config_state(args: &crate::cli::Args) -> Result<LoadedConfigState, String> {
    let loaded =
        load_config_with_diagnostics(args.config.as_deref()).map_err(|err| err.to_string())?;
    let mut config = loaded.config;
    apply_cli_overrides(args, &mut config)?;

    let mut warnings = loaded.diagnostics.deprecations;
    match has_legacy_profile_token_records() {
        Ok(true) => warnings.push(
            "Auth store uses deprecated profile-scoped login records; run `buddy login` to migrate to provider-scoped records before v0.4."
                .to_string(),
        ),
        Ok(false) => {}
        Err(err) => warnings.push(format!(
            "failed to inspect auth store for legacy credentials: {err}"
        )),
    }
    let resume_request = resume_request_from_command(args.command.as_ref())?;

    Ok(LoadedConfigState {
        config,
        warnings,
        resume_request,
    })
}

/// Apply CLI runtime overrides that intentionally outrank config files.
fn apply_cli_overrides(args: &crate::cli::Args, config: &mut Config) -> Result<(), String> {
    if let Some(model) = &args.model {
        if config.models.contains_key(model) {
            select_model_profile(config, model)
                .map_err(|err| format!("failed to select model profile `{model}`: {err}"))?;
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
    Ok(())
}

/// Validate runtime prerequisites and build execution/tools/agent wiring.
async fn prepare_runtime_setup(
    args: &crate::cli::Args,
    renderer: &dyn RenderSink,
    mut loaded: LoadedConfigState,
) -> Result<RuntimeSetup, String> {
    // Setup sequence:
    // 1) validate profile and execution flags,
    // 2) initialize execution context,
    // 3) render system prompt with tool target metadata,
    // 4) build tool registry and agent.
    if loaded.config.api.base_url.is_empty() {
        return Err(
            "No API base URL configured. Set models.<name>.api_base_url in buddy.toml or BUDDY_BASE_URL env var."
                .to_string(),
        );
    }
    let preflight = validate_active_profile_ready(&loaded.config)?;
    for warning in preflight.warnings {
        renderer.warn(&warning);
    }

    let is_exec_command = matches!(args.command.as_ref(), Some(cli::Command::Exec { .. }));
    match enforce_exec_shell_guardrails(
        is_exec_command,
        args.dangerously_auto_approve,
        &mut loaded.config.tools,
    ) {
        Ok(Some(warning)) => renderer.warn(&warning),
        Ok(None) => {}
        Err(msg) => return Err(msg),
    }
    validate_execution_target_flags(args, &loaded.config)?;

    let execution = initialize_execution_context(args, &loaded.config).await?;
    let capture_pane_enabled = execution.capture_pane_available();
    let tmux_management_enabled = execution.tmux_management_available();
    configure_system_prompt(
        &mut loaded.config,
        args,
        capture_pane_enabled,
        tmux_management_enabled,
    );
    let tool_setup = build_tools(
        &loaded.config,
        &execution,
        !is_exec_command,
        capture_pane_enabled,
    );
    let agent = Agent::new(loaded.config.clone(), tool_setup.tools);

    Ok(RuntimeSetup {
        config: loaded.config,
        execution,
        capture_pane_enabled,
        agent,
        resume_request: loaded.resume_request,
        shell_approval_rx: tool_setup.shell_approval_rx,
    })
}

/// Validate CLI execution-target flags against enabled tool capabilities.
fn validate_execution_target_flags(args: &crate::cli::Args, config: &Config) -> Result<(), String> {
    if (args.container.is_some() || args.ssh.is_some() || args.tmux.is_some())
        && !config.tools.shell_enabled
        && !config.tools.files_enabled
    {
        return Err(
            "Execution target flags (--container/--ssh/--tmux) require `run_shell` or file tools to be enabled. Enable `tools.shell_enabled` and/or `tools.files_enabled` in config."
                .to_string(),
        );
    }
    Ok(())
}

/// Build execution context from CLI target flags and current config.
async fn initialize_execution_context(
    args: &crate::cli::Args,
    config: &Config,
) -> Result<ExecutionContext, String> {
    let execution_tools_enabled = config.tools.shell_enabled || config.tools.files_enabled;
    let requested_tmux_session = args.tmux.clone().flatten();
    if !execution_tools_enabled {
        return Ok(ExecutionContext::local());
    }

    if let Some(container) = &args.container {
        return ExecutionContext::container_tmux(
            container.clone(),
            requested_tmux_session,
            &config.agent.name,
            config.tmux.max_sessions,
            config.tmux.max_panes,
        )
        .await
        .map_err(|err| format!("failed to initialize container execution: {err}"));
    }

    if let Some(target) = &args.ssh {
        return ExecutionContext::ssh(
            target.clone(),
            requested_tmux_session,
            &config.agent.name,
            config.tmux.max_sessions,
            config.tmux.max_panes,
        )
        .await
        .map_err(|err| format!("failed to initialize ssh execution: {err}"));
    }

    ExecutionContext::local_tmux(
        requested_tmux_session,
        &config.agent.name,
        config.tmux.max_sessions,
        config.tmux.max_panes,
    )
    .await
    .map_err(|err| format!("failed to initialize local tmux execution: {err}"))
}

/// Render and install the final system prompt string for this invocation.
fn configure_system_prompt(
    config: &mut Config,
    args: &crate::cli::Args,
    capture_pane_enabled: bool,
    tmux_management_enabled: bool,
) {
    let prompt_tool_names =
        enabled_tool_names(config, capture_pane_enabled, tmux_management_enabled);
    let custom_prompt = config.agent.system_prompt.trim().to_string();
    config.agent.system_prompt = render_system_prompt(SystemPromptParams {
        execution_target: if let Some(container) = args.container.as_deref() {
            ExecutionTarget::Container(container)
        } else if let Some(host) = args.ssh.as_deref() {
            ExecutionTarget::Ssh(host)
        } else {
            ExecutionTarget::Local
        },
        enabled_tools: prompt_tool_names,
        custom_instructions: (!custom_prompt.is_empty()).then_some(custom_prompt.as_str()),
    });
}

/// Register tools according to config flags and execution capabilities.
fn build_tools(
    config: &Config,
    execution: &ExecutionContext,
    interactive_mode: bool,
    capture_pane_enabled: bool,
) -> ToolSetup {
    let mut tools = ToolRegistry::new();
    let builtin_tool_names = default_builtin_tool_names(
        config.api.provider,
        &config.api.base_url,
        config.api.auth,
        &config.api.api_key,
        &config.api.model,
    );
    let builtin_web_search = builtin_tool_names.contains(&"web_search");
    let needs_tmux_management_approval =
        capture_pane_enabled && interactive_mode && execution.tmux_management_available();
    let needs_approval_broker = interactive_mode
        && ((config.tools.shell_enabled && config.tools.shell_confirm)
            || (config.tools.fetch_enabled && config.tools.fetch_confirm)
            || needs_tmux_management_approval);
    let (shell_approval_broker, shell_approval_rx) = if needs_approval_broker {
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
        if execution.tmux_management_available() {
            let shared = TmuxToolShared {
                execution: execution.clone(),
                confirm: true,
                approval: shell_approval_broker.clone(),
            };
            tools.register(TmuxCreateSessionTool {
                shared: shared.clone(),
            });
            tools.register(TmuxKillSessionTool {
                shared: shared.clone(),
            });
            tools.register(TmuxCreatePaneTool {
                shared: shared.clone(),
            });
            tools.register(TmuxKillPaneTool { shared });
        }
    }
    if config.tools.fetch_enabled {
        tools.register(FetchTool::new(
            Duration::from_secs(config.network.fetch_timeout_secs),
            config.tools.fetch_confirm,
            config.tools.fetch_allowed_domains.clone(),
            config.tools.fetch_blocked_domains.clone(),
            shell_approval_broker,
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
    if config.tools.search_enabled && !builtin_web_search {
        tools.register(WebSearchTool::new(Duration::from_secs(
            config.network.fetch_timeout_secs,
        )));
    }
    tools.register(TimeTool);

    ToolSetup {
        tools,
        shell_approval_rx,
    }
}

/// Handle `buddy login` health/reset/device-auth flow.
pub(crate) async fn run_login_flow(
    renderer: &dyn RenderSink,
    config: &Config,
    selector: Option<&str>,
    reset: bool,
    check: bool,
) -> Result<(), String> {
    // Login flow:
    // 1) resolve provider selector (provider-first with profile compatibility),
    // 2) surface lightweight status/check/reset behavior,
    // 3) execute provider-specific device login flow when needed.
    let selection = resolve_auth_provider_selector(config, selector, "login")?;
    if let Some(profile_name) = selection.legacy_profile.as_deref() {
        renderer.warn(&format!(
            "Using model profile selector `{profile_name}` for `/login` is deprecated. Use provider names like `openai`."
        ));
    }
    let provider = selection.provider_label;
    if provider != "openai" {
        return Err(format!(
            "provider `{provider}` does not support login auth. Use API key auth for this provider."
        ));
    }

    let health = {
        let mut progress = renderer.progress("checking saved login status");
        let result = provider_login_health(&provider)
            .map_err(|err| format!("failed to check existing login health: {err}"));
        progress.finish();
        result?
    };
    if check {
        renderer.section("login status");
        renderer.field("provider", &provider);
        renderer.field("logged_in", if health.has_tokens { "yes" } else { "no" });
        if health.has_tokens {
            renderer.detail("use `/logout openai` to clear saved login credentials.");
        }
        eprintln!();
        return Ok(());
    }

    if reset {
        let removed = reset_provider_tokens(&provider)
            .map_err(|err| format!("failed to reset saved login credentials: {err}"))?;
        if removed {
            renderer.section("logout");
            renderer.field("provider", &provider);
            renderer.detail("removed saved login credentials.");
            eprintln!();
        }
    } else if health.has_tokens {
        renderer.section("login");
        renderer.detail(&format!(
            "already logged into {provider}. Use `/logout {provider}` to log out."
        ));
        eprintln!();
        return Ok(());
    }

    let login = {
        let mut progress = renderer.progress("starting device login flow");
        let result = start_openai_device_login()
            .await
            .map_err(|err| format!("failed to start login flow: {err}"));
        progress.finish();
        result?
    };
    // Best-effort browser launch for convenience. The flow still works if this fails.
    let _ = try_open_browser(&login.verification_url);

    renderer.section(&format!(
        "logging you into {provider} via {}",
        login.verification_url
    ));
    renderer.field("device code", &login.user_code);
    renderer.detail("(open your browser and go to the url above, and enter the device code)");
    eprintln!();

    let tokens = {
        let _progress = renderer.progress("waiting for authorization");
        complete_openai_device_login(&login)
            .await
            .map_err(|err| format!("login failed: {err}"))?
    };
    save_provider_tokens(&provider, tokens)
        .map_err(|err| format!("failed to save login credentials: {err}"))?;

    renderer.section("login successful");
    renderer.field("provider", &provider);
    if config.api.auth != AuthMode::Login {
        renderer.detail(
            "active profile currently uses auth=api-key; set auth=login to use saved login tokens.",
        );
    }
    eprintln!();

    Ok(())
}

/// Handle `buddy logout` / `/logout` provider credential removal.
pub(crate) fn run_logout_flow(
    renderer: &dyn RenderSink,
    config: &Config,
    selector: Option<&str>,
) -> Result<(), String> {
    let selection = resolve_auth_provider_selector(config, selector, "logout")?;
    if let Some(profile_name) = selection.legacy_profile.as_deref() {
        renderer.warn(&format!(
            "Using model profile selector `{profile_name}` for `/logout` is deprecated. Use provider names like `openai`."
        ));
    }
    let provider = selection.provider_label;
    let removed = reset_provider_tokens(&provider)
        .map_err(|err| format!("failed to clear saved login credentials: {err}"))?;

    if removed {
        renderer.section("logout");
        renderer.field("provider", &provider);
        renderer.detail("saved login credentials removed.");
    } else {
        renderer.section("logout");
        renderer.field("provider", &provider);
        renderer.detail("no saved login credentials found.");
    }
    eprintln!();
    Ok(())
}

/// Return enabled tool identifiers for prompt rendering.
fn enabled_tool_names(
    config: &Config,
    capture_pane_enabled: bool,
    tmux_management_enabled: bool,
) -> Vec<&'static str> {
    let mut tools = Vec::new();
    if config.tools.shell_enabled {
        tools.push("run_shell");
    }
    if capture_pane_enabled {
        tools.push("tmux_capture_pane");
        tools.push("tmux_send_keys");
        if tmux_management_enabled {
            tools.push("tmux_create_session");
            tools.push("tmux_kill_session");
            tools.push("tmux_create_pane");
            tools.push("tmux_kill_pane");
        }
    }
    if config.tools.fetch_enabled {
        tools.push("fetch_url");
    }
    if config.tools.files_enabled {
        tools.push("read_file");
        tools.push("write_file");
    }
    let builtin_tool_names = default_builtin_tool_names(
        config.api.provider,
        &config.api.base_url,
        config.api.auth,
        &config.api.api_key,
        &config.api.model,
    );
    let builtin_web_search = builtin_tool_names.contains(&"web_search");
    if config.tools.search_enabled && !builtin_web_search {
        tools.push("web_search");
    }
    tools.extend(builtin_tool_names);
    tools.push("time");
    tools
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Clone, Default)]
    struct MockRenderer {
        /// Captured `(kind, message)` render events for assertions.
        entries: Arc<StdMutex<Vec<(String, String)>>>,
    }

    impl MockRenderer {
        /// Record one render event emitted through the `RenderSink` interface.
        fn record(&self, kind: &str, message: &str) {
            self.entries
                .lock()
                .expect("mock renderer lock")
                .push((kind.to_string(), message.to_string()));
        }

        /// Return true when a recorded event of `kind` contains `needle`.
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

        fn progress(&self, label: &str) -> buddy::ui::render::ProgressHandle {
            self.record("progress", label);
            Renderer::new(false).progress(label)
        }

        fn progress_with_metrics(
            &self,
            label: &str,
            _metrics: buddy::ui::render::ProgressMetrics,
        ) -> buddy::ui::render::ProgressHandle {
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
        // Non-interactive exec must fail when shell confirmations are still required.
        let mut tools = ToolsConfig {
            shell_enabled: true,
            shell_confirm: true,
            ..ToolsConfig::default()
        };
        let err = enforce_exec_shell_guardrails(true, false, &mut tools).unwrap_err();
        assert!(
            err.contains("--dangerously-auto-approve"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn exec_shell_guardrails_auto_approve_disables_shell_confirm() {
        // Dangerous override should disable confirm and return an explicit warning.
        let mut tools = ToolsConfig {
            shell_enabled: true,
            shell_confirm: true,
            ..ToolsConfig::default()
        };
        let warning = enforce_exec_shell_guardrails(true, true, &mut tools)
            .expect("guardrail should allow override")
            .expect("warning expected");
        assert!(warning.contains("Dangerous mode"));
        assert!(!tools.shell_confirm);
    }

    #[test]
    fn parse_approval_decision_supports_yes_no_and_default_deny() {
        // Approval parser should accept y/yes/n/empty and reject unrelated input.
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
    fn parse_shell_tool_result_extracts_code_stdout_and_stderr() {
        // Legacy text shell payloads should parse into structured exit/stdout/stderr fields.
        let parsed = parse_shell_tool_result("exit code: 7\nstdout:\na\nb\nstderr:\nwarn")
            .expect("shell output should parse");
        assert_eq!(parsed.exit_code, 7);
        assert_eq!(parsed.stdout, "a\nb");
        assert_eq!(parsed.stderr, "warn");
    }

    #[test]
    fn parse_shell_tool_result_rejects_unexpected_shape() {
        // Non-shell payloads should not produce false-positive parses.
        assert!(parse_shell_tool_result("not shell output").is_none());
    }

    #[test]
    fn parse_shell_tool_result_extracts_from_enveloped_json() {
        // JSON-enveloped shell payloads should unwrap and parse successfully.
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
        // Envelope helper should return plain string results without metadata wrapper.
        let result = serde_json::json!({
            "harness_timestamp": { "source": "harness", "unix_millis": 123 },
            "result": "hello"
        })
        .to_string();
        assert_eq!(tool_result_display_text(&result), "hello");
    }

    #[test]
    fn background_liveness_line_includes_running_task_state() {
        // Liveness line should include task id and running-state wording.
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
        // Waiting-approval transition should only apply to the targeted task id.
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
        ));
        assert!(task_is_waiting_for_approval(&tasks, 2));
        assert!(!task_is_waiting_for_approval(&tasks, 1));
    }

    #[test]
    fn parse_duration_arg_supports_common_units() {
        // Duration parser should accept seconds/minutes/hours/millis shorthands.
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
        // `/approve` parser should support named modes and duration-based policy.
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
        // Timeout command should require explicit id when multiple tasks are running.
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
        // Timeout command may target the only running task when id is omitted.
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

    #[test]
    fn openai_builtin_tool_names_enabled_for_reasoning_profiles() {
        // GPT-5/Codex profiles should expose OpenAI-native web + python tools.
        let names = default_builtin_tool_names(
            ModelProvider::Openai,
            "https://api.openai.com/v1",
            AuthMode::ApiKey,
            "sk-test",
            "gpt-5.3-codex",
        );
        assert_eq!(names, vec!["web_search", "code_interpreter"]);
    }

    #[test]
    fn openai_builtin_tool_names_disabled_for_non_openai_profiles() {
        // Non-OpenAI providers should not advertise OpenAI built-in tools.
        let names = default_builtin_tool_names(
            ModelProvider::Openrouter,
            "https://openrouter.ai/api/v1",
            AuthMode::ApiKey,
            "sk-test",
            "gpt-5.3-codex",
        );
        assert!(names.is_empty());
    }

    #[test]
    fn openai_builtin_tool_names_disabled_for_login_auth_mode() {
        // ChatGPT/Codex login runtime rejects these built-ins.
        let names = default_builtin_tool_names(
            ModelProvider::Openai,
            "https://api.openai.com/v1",
            AuthMode::Login,
            "",
            "gpt-5.3-codex",
        );
        assert!(names.is_empty());
    }

    #[tokio::test]
    async fn handle_session_command_new_submits_runtime_command() {
        // `/session new` should enqueue a runtime `SessionNew` command.
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
        // `/session resume` without an id should warn and avoid runtime submission.
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
        // `/session resume last` should warn when no saved sessions exist.
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
        // `/model <selector>` should submit `SwitchModel` with resolved profile.
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut config = Config::default();

        handle_model_command(
            &renderer,
            &mut config,
            &runtime,
            Some("openrouter-deepseek"),
            None,
        )
        .await;

        let command = rx.recv().await.expect("command expected");
        match command {
            RuntimeCommand::SwitchModel {
                profile,
                reasoning_effort,
                auth_override,
                api_key_env_override,
                clear_key_sources,
            } => {
                assert_eq!(profile, "openrouter-deepseek");
                assert!(reasoning_effort.is_none());
                assert!(auth_override.is_none());
                assert!(api_key_env_override.is_none());
                assert!(!clear_key_sources);
            }
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(
            !renderer.saw("section", "switched model profile: openrouter-deepseek"),
            "model summary should be rendered after runtime acknowledgement"
        );
    }

    #[tokio::test]
    async fn handle_model_command_warns_on_unknown_selector() {
        // Unknown model selector should warn and avoid runtime command dispatch.
        let (tx, mut rx) = mpsc::channel(4);
        let runtime = BuddyRuntimeHandle { commands: tx };
        let renderer = MockRenderer::default();
        let mut config = Config::default();

        handle_model_command(
            &renderer,
            &mut config,
            &runtime,
            Some("missing-profile"),
            None,
        )
        .await;

        assert!(
            renderer.saw("warn", "Unknown model profile"),
            "expected unknown-profile warning"
        );
        assert!(rx.try_recv().is_err(), "no runtime command expected");
    }

    #[test]
    fn process_runtime_events_routes_warning_to_injected_renderer() {
        // Runtime warning events should route through the injected render sink.
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

        let mut runtime_event_context = ProcessRuntimeEventsContext {
            renderer: &renderer,
            background_tasks: &mut background_tasks,
            completed_tasks: &mut completed_tasks,
            pending_approval: &mut pending_approval,
            config: &mut config,
            active_session: &mut active_session,
            runtime_context: &mut runtime_context,
        };
        process_runtime_events(&mut events, &mut runtime_event_context);

        assert!(
            renderer.saw("warn", "demo warning"),
            "runtime warning should flow through render trait"
        );
    }
}
