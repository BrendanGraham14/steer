use anyhow::Result;
use std::collections::HashMap;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::api::messages::Message as ApiMessage;
use crate::app::agent_executor::AgentExecutorError;
use crate::app::cancellation::ActiveTool;
use crate::tools::ToolError;

#[derive(Debug)]
pub enum TaskOutcome {
    AgentOperationComplete {
        result: Result<ApiMessage, AgentExecutorError>,
    },
    DispatchAgentResult {
        result: Result<String, ToolError>,
    },
}

// Holds the state for a single, cancellable user-initiated operation
pub struct OpContext {
    pub cancel_token: CancellationToken,
    // Tasks now return TaskOutcome
    pub tasks: JoinSet<TaskOutcome>,
    // Track active tools by ID -> tool info (Kept for cancellation info)
    pub active_tools: HashMap<String, ActiveTool>,
    // Removed: agent_event_receiver
    // Removed: pending_tool_calls, expected_tool_results, api_call_in_progress
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
            active_tools: HashMap::new(),
            // Removed: agent_event_receiver: None,
        }
    }

    // Removed: start_api_call, complete_api_call

    pub fn add_active_tool(&mut self, id: String, name: String) {
        self.active_tools
            .insert(id.clone(), ActiveTool { id, name });
    }

    pub fn remove_active_tool(&mut self, id: &str) -> Option<ActiveTool> {
        self.active_tools.remove(id)
    }

    pub fn has_activity(&self) -> bool {
        !self.tasks.is_empty() || !self.active_tools.is_empty()
    }

    pub async fn cancel_and_shutdown(&mut self) {
        self.cancel_token.cancel();
        self.tasks.shutdown().await;
        self.active_tools.clear();
        // Removed: self.agent_event_receiver = None;
    }
}
