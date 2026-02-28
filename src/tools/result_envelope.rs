//! Shared tool-result envelope helpers.
//!
//! All tools return this JSON envelope so the model always gets a harness-side
//! timestamp alongside tool-specific output payload.

use crate::error::ToolError;
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Minimal harness clock snapshot attached to every tool response.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HarnessTimestamp {
    pub source: &'static str,
    pub unix_millis: u64,
}

impl HarnessTimestamp {
    pub fn now() -> Self {
        let unix_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            source: "harness",
            unix_millis,
        }
    }
}

/// Standard tool-response envelope.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolResultEnvelope<T>
where
    T: Serialize,
{
    pub harness_timestamp: HarnessTimestamp,
    pub result: T,
}

/// Wrap a tool payload in the standard JSON envelope.
pub fn wrap_result<T>(result: T) -> Result<String, ToolError>
where
    T: Serialize,
{
    let envelope = ToolResultEnvelope {
        harness_timestamp: HarnessTimestamp::now(),
        result,
    };
    serde_json::to_string(&envelope).map_err(|e| {
        ToolError::ExecutionFailed(format!("failed to serialize tool result envelope: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_result_contains_result_and_timestamp() {
        let json = wrap_result("ok").expect("envelope");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(value["result"], "ok");
        assert_eq!(value["harness_timestamp"]["source"], "harness");
        assert!(value["harness_timestamp"]["unix_millis"].as_u64().is_some());
    }
}
