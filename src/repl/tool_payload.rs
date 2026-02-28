//! Tool payload parsing and display helpers shared by CLI rendering paths.

use crate::textutil::truncate_with_suffix_by_chars;
use serde_json::Value;

/// Structured `run_shell` tool output shape used by CLI rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellToolResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Parse shell result output from either structured or legacy payload formats.
pub fn parse_shell_tool_result(result: &str) -> Option<ShellToolResult> {
    if let Some(payload) = parse_tool_result_payload(result) {
        if let Some(parsed) = parse_structured_shell_payload(&payload) {
            return Some(parsed);
        }
        if let Some(text) = payload.as_str() {
            return parse_legacy_shell_payload(text);
        }
    }
    parse_legacy_shell_payload(result)
}

fn parse_structured_shell_payload(payload: &Value) -> Option<ShellToolResult> {
    let obj = payload.as_object()?;
    Some(ShellToolResult {
        exit_code: obj.get("exit_code")?.as_i64()? as i32,
        stdout: obj.get("stdout")?.as_str()?.to_string(),
        stderr: obj.get("stderr")?.as_str()?.to_string(),
    })
}

fn parse_legacy_shell_payload(result: &str) -> Option<ShellToolResult> {
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

fn parse_tool_result_payload(result: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(result).ok()?;
    let object = value.as_object()?;
    object.get("result").cloned()
}

/// Extract a display-safe tool result text from envelope or raw string output.
pub fn tool_result_display_text(result: &str) -> String {
    if let Some(payload) = parse_tool_result_payload(result) {
        if let Some(text) = payload.as_str() {
            return text.to_string();
        }
        return payload.to_string();
    }
    result.to_string()
}

/// Parse a string argument from a JSON tool-arguments object.
pub fn parse_tool_arg(args: &str, key: &str) -> Option<String> {
    let value: Value = serde_json::from_str(args).ok()?;
    value.get(key)?.as_str().map(str::to_string)
}

/// Quote-escaped single-line preview used in human-oriented activity output.
pub fn quote_preview(text: &str, max_len: usize) -> String {
    truncate_preview(text, max_len).replace('"', "\\\"")
}

/// Single-line truncation helper that also flattens newlines to spaces.
pub fn truncate_preview(text: &str, max_len: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    truncate_with_suffix_by_chars(&flat, max_len, "...")
}
