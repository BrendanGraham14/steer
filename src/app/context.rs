use anyhow::Result;
use std::collections::HashMap;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::api::{CompletionResponse, ToolCall};
use crate::app::cancellation::ActiveTool;

// New Enum for Task Outcomes
#[derive(Debug)]
pub enum TaskOutcome {
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        result: Result<String>, // Tool output string or error
    },
    ApiResponse {
        // Identifier might be needed later if multiple API calls can be concurrent within one OpContext
        result: Result<CompletionResponse, String>, // API Response or serialized error
    },
    // Add other variants here if needed, e.g.:
    // CompactResult { result: Result<()> },
}

// Holds the state for a single, cancellable user-initiated operation
pub struct OpContext {
    pub cancel_token: CancellationToken,
    // Tasks now return TaskOutcome
    pub tasks: JoinSet<TaskOutcome>,
    // Store pending approvals within the operation's context
    pub pending_tool_calls: HashMap<String, ToolCall>,
    // Track expected tool results for the current step
    pub expected_tool_results: usize,
    // Track active tools by ID -> tool info
    pub active_tools: HashMap<String, ActiveTool>,
    // Flag indicating whether an API call is in progress
    pub api_call_in_progress: bool,
}

impl Default for OpContext {
    fn default() -> Self {
        Self::new()
    }
}

impl OpContext {
    pub fn new() -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            tasks: JoinSet::new(),
            pending_tool_calls: HashMap::new(),
            expected_tool_results: 0,
            active_tools: HashMap::new(),
            api_call_in_progress: true, // Start with API call in progress
        }
    }

    // Mark an API call as started
    pub fn start_api_call(&mut self) {
        self.api_call_in_progress = true;
    }

    // Mark an API call as completed
    pub fn complete_api_call(&mut self) {
        self.api_call_in_progress = false;
    }

    // Adds a tool to the active tools map
    pub fn add_active_tool(&mut self, id: String, name: String) {
        self.active_tools
            .insert(id.clone(), ActiveTool { id, name });
    }

    // Removes a tool from the active tools map
    pub fn remove_active_tool(&mut self, id: &str) -> Option<ActiveTool> {
        self.active_tools.remove(id)
    }

    // Check if we have any active operations
    pub fn has_activity(&self) -> bool {
        self.api_call_in_progress
            || !self.active_tools.is_empty()
            || !self.pending_tool_calls.is_empty()
    }

    // Convenience method to cancel the operation and shut down tasks
    pub async fn cancel_and_shutdown(&mut self) {
        self.cancel_token.cancel();
        // Clear pending calls as they are now irrelevant
        self.pending_tool_calls.clear();
        self.tasks.shutdown().await;
    }
}
