use crate::app::Message;
use crate::app::agent_executor::ApprovalDecision;
use tokio::sync::oneshot;
use tools::ToolCall as ApiToolCall;

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
    /// Restore a message to the conversation (used when resuming sessions)
    RestoreMessage(Message),
    /// Pre-approve tools for the session (used when resuming sessions)
    PreApproveTools(Vec<String>),
    /// Request to send the current conversation state
    /// Used by TUI to populate display after session restoration
    GetCurrentConversation,
}
