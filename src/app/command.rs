use serde::{Deserialize, Serialize};

/// Defines messages the TUI can send *to* the `App` actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
