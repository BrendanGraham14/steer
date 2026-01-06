use std::collections::HashMap;
use std::time::Instant;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid;

#[derive(Debug)]
pub enum TaskOutcome {
    DispatchAgentResult {
        result: std::result::Result<String, steer_tools::ToolError>,
    },
    BashCommandComplete {
        op_id: uuid::Uuid,
        command: String,
        start_time: Instant,
        result: std::result::Result<steer_tools::ToolResult, steer_tools::ToolError>,
    },
}

// Holds the state for a single, cancellable user-initiated operation
pub struct OpContext {
    pub cancel_token: CancellationToken,
    // Tasks now return TaskOutcome
    pub tasks: JoinSet<TaskOutcome>,
    // Track active tools by tool_call_id -> (op_id, start_time, tool_name)
    pub active_tools: HashMap<String, (uuid::Uuid, Instant, String)>,
    // Track the main operation ID if this context is for a Started/Finished operation
    pub operation_id: Option<uuid::Uuid>,
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
            operation_id: None,
            // Removed: agent_event_receiver: None,
        }
    }

    pub fn new_with_id(op_id: uuid::Uuid) -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            tasks: JoinSet::new(),
            active_tools: HashMap::new(),
            operation_id: Some(op_id),
        }
    }

    // Removed: start_api_call, complete_api_call

    pub fn add_active_tool(&mut self, id: String, op_id: uuid::Uuid, name: String) {
        self.active_tools.insert(id, (op_id, Instant::now(), name));
    }

    pub fn remove_active_tool(&mut self, id: &str) -> Option<(uuid::Uuid, Instant, String)> {
        self.active_tools.remove(id)
    }

    pub fn has_activity(&self) -> bool {
        !self.tasks.is_empty() || !self.active_tools.is_empty()
    }

    pub async fn cancel_and_shutdown(&mut self) {
        self.cancel_token.cancel();
        self.tasks.shutdown().await;
        self.active_tools.clear();
    }
}
