use crate::api::tools::ToolCall as ApiToolCall;
use crate::app::agent_executor::ApprovalDecision;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Defines messages the TUI can send *to* the `App` actor.
#[derive(Debug)]
pub enum AppCommand {
    /// Send a user's message text for processing.
    ProcessUserInput(String),
    /// Handle the user's decision on a tool approval request.
    HandleToolResponse {
        id: String,
        approved: bool,
        always: bool,
    },
    /// Execute a slash command.
    ExecuteCommand(String),
    /// Cancel processing.
    CancelProcessing,
    /// Signal for graceful shutdown.
    Shutdown,
    /// Internal command for tool executor callback to request approval
    RequestToolApprovalInternal {
        tool_call: ApiToolCall,
        responder: oneshot::Sender<ApprovalDecision>,
    },
}
