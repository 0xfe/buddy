//! Harness clock tool.
//!
//! Exposes the harness-observed current wall clock time in several common
//! machine-readable formats so the model can reason about deadlines and timing.

use async_trait::async_trait;
use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Tool that returns current harness wall-clock time in multiple formats.
pub struct TimeTool;

#[async_trait]
impl Tool for TimeTool {
    fn name(&self) -> &'static str {
        "time"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Return the current wall-clock time as recorded by the harness running this agent (not by the model, and not by remote shells). Includes Unix epoch and common UTC text formats."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let parsed: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        if !parsed.is_object() {
            return Err(ToolError::InvalidArguments(
                "arguments must be a JSON object".into(),
            ));
        }

        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| {
            ToolError::ExecutionFailed(format!("failed to read harness clock: {e}"))
        })?;
        let snapshot = build_snapshot(now);
        serde_json::to_string_pretty(&snapshot).map_err(|e| {
            ToolError::ExecutionFailed(format!("failed to serialize time snapshot: {e}"))
        })
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct TimeSnapshot {
    source: &'static str,
    note: &'static str,
    unix_seconds: u64,
    unix_millis: u64,
    unix_nanos: u64,
    iso_8601_utc: String,
    iso_8601_millis_utc: String,
    rfc_2822_utc: String,
    date_utc: String,
    time_utc: String,
}

fn build_snapshot(since_epoch: Duration) -> TimeSnapshot {
    let unix_seconds = since_epoch.as_secs();
    let unix_millis = since_epoch
        .as_secs()
        .saturating_mul(1000)
        .saturating_add((since_epoch.subsec_nanos() / 1_000_000) as u64);
    let unix_nanos = since_epoch
        .as_secs()
        .saturating_mul(1_000_000_000)
        .saturating_add(since_epoch.subsec_nanos() as u64);

    let utc = format_utc(unix_seconds as i64, since_epoch.subsec_nanos());

    TimeSnapshot {
        source: "harness",
        note: "This is the harness-recorded wall clock time for this agent process.",
        unix_seconds,
        unix_millis,
        unix_nanos,
        iso_8601_utc: format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            utc.year, utc.month, utc.day, utc.hour, utc.minute, utc.second
        ),
        iso_8601_millis_utc: format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
            utc.year,
            utc.month,
            utc.day,
            utc.hour,
            utc.minute,
            utc.second,
            utc.nanosecond / 1_000_000
        ),
        rfc_2822_utc: format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} +0000",
            weekday_name(utc.weekday_index),
            utc.day,
            month_name(utc.month),
            utc.year,
            utc.hour,
            utc.minute,
            utc.second
        ),
        date_utc: format!("{:04}-{:02}-{:02}", utc.year, utc.month, utc.day),
        time_utc: format!("{:02}:{:02}:{:02}", utc.hour, utc.minute, utc.second),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UtcFields {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    nanosecond: u32,
    weekday_index: u32,
}

fn format_utc(unix_seconds: i64, nanosecond: u32) -> UtcFields {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400) as u32;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    // Unix epoch (1970-01-01) was a Thursday. If Sunday=0, Thursday=4.
    let weekday_index = (days + 4).rem_euclid(7) as u32;

    UtcFields {
        year,
        month,
        day,
        hour,
        minute,
        second,
        nanosecond,
        weekday_index,
    }
}

fn civil_from_days(days_since_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn weekday_name(index: u32) -> &'static str {
    match index {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        _ => "Sat",
    }
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_time() {
        assert_eq!(TimeTool.name(), "time");
    }

    #[test]
    fn build_snapshot_formats_epoch_start() {
        let snapshot = build_snapshot(Duration::from_secs(0));
        assert_eq!(snapshot.unix_seconds, 0);
        assert_eq!(snapshot.date_utc, "1970-01-01");
        assert_eq!(snapshot.time_utc, "00:00:00");
        assert_eq!(snapshot.iso_8601_utc, "1970-01-01T00:00:00Z");
        assert_eq!(snapshot.rfc_2822_utc, "Thu, 01 Jan 1970 00:00:00 +0000");
    }

    #[test]
    fn build_snapshot_handles_known_timestamp() {
        let snapshot = build_snapshot(Duration::from_secs(1_709_130_123));
        assert_eq!(snapshot.date_utc, "2024-02-28");
        assert_eq!(snapshot.time_utc, "14:22:03");
        assert_eq!(snapshot.iso_8601_utc, "2024-02-28T14:22:03Z");
    }
}
