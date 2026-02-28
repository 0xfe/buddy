//! Tool runtime event handlers.

use buddy::ui::render::RenderSink;
use buddy::runtime::ToolEvent;

use crate::cli_event_renderer::RuntimeEventRenderContext;
use crate::repl_support::{
    parse_shell_tool_result, parse_tool_arg, quote_preview, tool_result_display_text,
    truncate_preview,
};

pub(in crate::cli_event_renderer) fn handle_tool(
    ctx: &mut RuntimeEventRenderContext<'_>,
    event: ToolEvent,
) {
    match event {
        ToolEvent::CallRequested { .. } => {}
        ToolEvent::CallStarted { task, name, detail } => {
            if name == "run_shell" {
                // Shell commands already get explicit approval/request rendering (when
                // enabled) and a structured final result block. Suppress duplicate
                // "running run_shell" lifecycle chatter.
                return;
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
                return;
            }
            ctx.renderer.activity(&format!(
                "task #{} {name} output: {}",
                task.task_id,
                truncate_preview(&chunk, 120)
            ));
        }
        ToolEvent::StderrChunk { task, name, chunk } => {
            if name == "run_shell" {
                return;
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
                return;
            }
            ctx.renderer.activity(&format!(
                "task #{} {name}: {}",
                task.task_id,
                truncate_preview(&message, 120)
            ));
        }
        ToolEvent::Completed { task, name, detail } => {
            if name == "run_shell" {
                return;
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
        } => render_tool_result(ctx.renderer, task.task_id, &name, &arguments_json, &result),
    }
}

fn render_tool_result(
    renderer: &dyn RenderSink,
    task_id: u64,
    name: &str,
    args: &str,
    result: &str,
) {
    let display_result = tool_result_display_text(result);
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
            if display_result.contains("command dispatched to tmux pane") {
                renderer.activity(&format!(
                    "task #{task_id} run_shell: {}",
                    truncate_preview(&display_result, 140)
                ));
                eprintln!();
                return;
            }
        }
        "read_file" => {
            let path = parse_tool_arg(args, "path").unwrap_or_else(|| "<path>".to_string());
            renderer.activity(&format!("task #{task_id} read {path}"));
            renderer.tool_output_block(&display_result, Some(path.as_str()));
            return;
        }
        "write_file" => {
            renderer.activity(&format!(
                "task #{task_id} write_file: {}",
                truncate_preview(&display_result, 120)
            ));
            eprintln!();
            return;
        }
        "fetch_url" => {
            let url = parse_tool_arg(args, "url").unwrap_or_else(|| "<url>".to_string());
            renderer.activity(&format!(
                "task #{task_id} fetched {url}: \"{}\"",
                quote_preview(&display_result, 120)
            ));
            eprintln!();
            return;
        }
        "web_search" => {
            let query = parse_tool_arg(args, "query").unwrap_or_else(|| "<query>".to_string());
            renderer.activity(&format!(
                "task #{task_id} searched \"{}\": \"{}\"",
                truncate_preview(&query, 64),
                quote_preview(&display_result, 120)
            ));
            eprintln!();
            return;
        }
        "capture-pane" => {
            let target = parse_tool_arg(args, "target").unwrap_or_else(|| "<default>".to_string());
            renderer.activity(&format!("task #{task_id} captured pane {target}"));
            renderer.command_output_block(&display_result);
            return;
        }
        "time" => {
            renderer.activity(&format!(
                "task #{task_id} read harness time: \"{}\"",
                quote_preview(&display_result, 120)
            ));
            eprintln!();
            return;
        }
        "send-keys" => {
            renderer.activity(&format!(
                "task #{task_id} send-keys: {}",
                truncate_preview(&display_result, 120)
            ));
            eprintln!();
            return;
        }
        _ => {}
    }

    renderer.activity(&format!(
        "task #{task_id} {name}: {}",
        truncate_preview(&display_result, 120)
    ));
    eprintln!();
}
