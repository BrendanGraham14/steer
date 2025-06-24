use crate::app::Message;
use crate::app::agent_executor::ApprovalDecision;
use std::collections::HashSet;
use tokio::sync::oneshot;
use tools::ToolCall;

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
    /// Execute a bash command directly (bypassing AI)
    ExecuteBashCommand { command: String },
    /// Cancel processing.
    CancelProcessing,
    /// Signal for graceful shutdown.
    Shutdown,
    /// Internal command for tool executor callback to request approval
    RequestToolApprovalInternal {
        tool_call: ToolCall,
        responder: oneshot::Sender<ApprovalDecision>,
    },
    /// Restore conversation state when resuming a session
    RestoreConversation {
        messages: Vec<Message>,
        approved_tools: HashSet<String>,
    },
    /// Request to send the current conversation state
    /// Used by TUI to populate display after session restoration
    GetCurrentConversation,
}
