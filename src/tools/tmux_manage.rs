//! First-class managed tmux session/pane lifecycle tools.
//!
//! These tools expose explicit tmux management primitives while enforcing
//! buddy ownership boundaries through `ExecutionContext`.

use async_trait::async_trait;
use serde::Deserialize;

use super::execution::ExecutionContext;
use super::result_envelope::wrap_result;
use super::shell::{RiskLevel, ShellApprovalBroker, ShellApprovalMetadata};
use super::{Tool, ToolContext};
use crate::error::ToolError;
use crate::types::{FunctionDefinition, ToolDefinition};

/// Shared approval policy and execution context for tmux management tools.
#[derive(Clone)]
pub struct TmuxToolShared {
    /// Backing execution context for tmux operations.
    pub execution: ExecutionContext,
    /// Whether to require approval before tmux mutations.
    pub confirm: bool,
    /// Approval broker used by interactive runtime mode.
    pub approval: Option<ShellApprovalBroker>,
}

#[derive(Deserialize)]
struct MetadataArgs {
    /// Declared risk classification for this action.
    risk: RiskLevel,
    /// Whether action mutates state.
    mutation: bool,
    /// Whether action involves privilege escalation.
    privesc: bool,
    /// Human rationale for this operation.
    why: String,
}

impl MetadataArgs {
    fn to_metadata(&self) -> Result<ShellApprovalMetadata, ToolError> {
        ShellApprovalMetadata::new(self.risk, self.mutation, self.privesc, self.why.clone())
    }
}

async fn maybe_request_approval(
    shared: &TmuxToolShared,
    command: String,
    metadata: ShellApprovalMetadata,
) -> Result<(), ToolError> {
    if !shared.confirm {
        return Ok(());
    }
    let Some(approval) = &shared.approval else {
        return Err(ToolError::ExecutionFailed(
            "tmux management approval UI is unavailable".into(),
        ));
    };
    let approved = approval.request(command, Some(metadata)).await?;
    if approved {
        Ok(())
    } else {
        Err(ToolError::ExecutionFailed(
            "tmux management operation denied by user".into(),
        ))
    }
}

/// Tool: create or reuse a managed tmux session.
pub struct TmuxCreateSessionTool {
    /// Shared context and approval wiring.
    pub shared: TmuxToolShared,
}

#[derive(Deserialize)]
struct CreateSessionArgs {
    /// Requested session selector (canonicalized with buddy owner prefix).
    session: String,
    /// Approval metadata.
    #[serde(flatten)]
    meta: MetadataArgs,
}

#[async_trait]
impl Tool for TmuxCreateSessionTool {
    fn name(&self) -> &'static str {
        "tmux-create-session"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Create or reuse a buddy-managed tmux session and ensure its shared pane is ready.".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session": { "type": "string", "description": "Session name (logical name or full managed session name)." },
                        "risk": { "type": "string", "enum": ["low", "medium", "high"] },
                        "mutation": { "type": "boolean" },
                        "privesc": { "type": "boolean" },
                        "why": { "type": "string" }
                    },
                    "required": ["session", "risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: CreateSessionArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let metadata = args.meta.to_metadata()?;
        maybe_request_approval(
            &self.shared,
            format!("tmux create-session {}", args.session),
            metadata,
        )
        .await?;
        let result = self
            .shared
            .execution
            .create_tmux_session(args.session)
            .await?;
        wrap_result(result)
    }
}

/// Tool: kill a managed tmux session.
pub struct TmuxKillSessionTool {
    /// Shared context and approval wiring.
    pub shared: TmuxToolShared,
}

#[derive(Deserialize)]
struct KillSessionArgs {
    /// Session name (logical or full managed).
    session: String,
    /// Approval metadata.
    #[serde(flatten)]
    meta: MetadataArgs,
}

#[async_trait]
impl Tool for TmuxKillSessionTool {
    fn name(&self) -> &'static str {
        "tmux-kill-session"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description:
                    "Kill a buddy-managed tmux session (cannot kill the default shared session)."
                        .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session": { "type": "string", "description": "Session name (logical name or full managed session name)." },
                        "risk": { "type": "string", "enum": ["low", "medium", "high"] },
                        "mutation": { "type": "boolean" },
                        "privesc": { "type": "boolean" },
                        "why": { "type": "string" }
                    },
                    "required": ["session", "risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: KillSessionArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let metadata = args.meta.to_metadata()?;
        maybe_request_approval(
            &self.shared,
            format!("tmux kill-session {}", args.session),
            metadata,
        )
        .await?;
        let result = self
            .shared
            .execution
            .kill_tmux_session(args.session)
            .await?;
        wrap_result(result)
    }
}

/// Tool: create or reuse a managed pane in a session.
pub struct TmuxCreatePaneTool {
    /// Shared context and approval wiring.
    pub shared: TmuxToolShared,
}

#[derive(Deserialize)]
struct CreatePaneArgs {
    /// Optional session selector; defaults to managed shared session.
    session: Option<String>,
    /// Pane name (logical or full managed pane title).
    pane: String,
    /// Approval metadata.
    #[serde(flatten)]
    meta: MetadataArgs,
}

#[async_trait]
impl Tool for TmuxCreatePaneTool {
    fn name(&self) -> &'static str {
        "tmux-create-pane"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Create or reuse a buddy-managed tmux pane in a managed session."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session": { "type": "string", "description": "Optional session selector. Defaults to managed shared session." },
                        "pane": { "type": "string", "description": "Pane name (logical name or full managed pane title)." },
                        "risk": { "type": "string", "enum": ["low", "medium", "high"] },
                        "mutation": { "type": "boolean" },
                        "privesc": { "type": "boolean" },
                        "why": { "type": "string" }
                    },
                    "required": ["pane", "risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: CreatePaneArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let metadata = args.meta.to_metadata()?;
        maybe_request_approval(
            &self.shared,
            format!(
                "tmux create-pane {}:{}",
                args.session.as_deref().unwrap_or("<default>"),
                args.pane
            ),
            metadata,
        )
        .await?;
        let result = self
            .shared
            .execution
            .create_tmux_pane(args.session, args.pane)
            .await?;
        wrap_result(result)
    }
}

/// Tool: kill one managed pane in a managed session.
pub struct TmuxKillPaneTool {
    /// Shared context and approval wiring.
    pub shared: TmuxToolShared,
}

#[derive(Deserialize)]
struct KillPaneArgs {
    /// Optional session selector; defaults to managed shared session.
    session: Option<String>,
    /// Pane name (logical or full managed pane title).
    pane: String,
    /// Approval metadata.
    #[serde(flatten)]
    meta: MetadataArgs,
}

#[async_trait]
impl Tool for TmuxKillPaneTool {
    fn name(&self) -> &'static str {
        "tmux-kill-pane"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: FunctionDefinition {
                name: self.name().into(),
                description: "Kill one buddy-managed tmux pane (default shared pane is protected)."
                    .into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "session": { "type": "string", "description": "Optional session selector. Defaults to managed shared session." },
                        "pane": { "type": "string", "description": "Pane name (logical name or full managed pane title)." },
                        "risk": { "type": "string", "enum": ["low", "medium", "high"] },
                        "mutation": { "type": "boolean" },
                        "privesc": { "type": "boolean" },
                        "why": { "type": "string" }
                    },
                    "required": ["pane", "risk", "mutation", "privesc", "why"]
                }),
            },
        }
    }

    async fn execute(&self, arguments: &str, _context: &ToolContext) -> Result<String, ToolError> {
        let args: KillPaneArgs = serde_json::from_str(arguments)
            .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;
        let metadata = args.meta.to_metadata()?;
        maybe_request_approval(
            &self.shared,
            format!(
                "tmux kill-pane {}:{}",
                args.session.as_deref().unwrap_or("<default>"),
                args.pane
            ),
            metadata,
        )
        .await?;
        let result = self
            .shared
            .execution
            .kill_tmux_pane(args.session, args.pane)
            .await?;
        wrap_result(result)
    }
}
