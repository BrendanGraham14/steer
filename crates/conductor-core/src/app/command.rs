use crate::app::Message;
use crate::app::agent_executor::ApprovalDecision;
use crate::app::conversation::AppCommandType;
use conductor_tools::ToolCall;
use std::collections::HashSet;
use tokio::sync::oneshot;

/// Tool-specific approval payload for different types of tool approvals
#[derive(Debug, Clone)]
pub enum ApprovalType {
    /// Denied approval for this specific tool call
    Denied,
    /// One-time approval for this specific tool call
    Once,
    /// Always approve this entire tool
    AlwaysTool,
    /// Always approve this specific bash command pattern
    AlwaysBashPattern(String),
}

/// Defines messages the TUI can send *to* the `App` actor.
#[derive(Debug)]
pub enum AppCommand {
    /// Send a user's message text for processing.
    ProcessUserInput(String),
    /// Edit a previous message, creating a new branch
    EditMessage {
        /// ID of the message to edit. The new message will share the same parent.
        message_id: String,
        /// New content for the edited message
        new_content: String,
    },
    /// Handle the user's decision on a tool approval request.
    HandleToolResponse { id: String, approval: ApprovalType },
    /// Execute a slash command.
    ExecuteCommand(AppCommandType),
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
        approved_bash_patterns: HashSet<String>,
        active_message_id: Option<String>,
    },
    /// Request to send the current conversation state
    /// Used by TUI to populate display after session restoration
    GetCurrentConversation,
    /// Request to list workspace files
    RequestWorkspaceFiles,
}
