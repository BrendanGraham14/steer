use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::api::ToolCall;
use crate::app::ToolExecutor;
use conductor_tools::result::ToolResult;

// Removed the old execute_tool_with_context function.

/// Executes the core logic of a tool call, handling cancellation.
/// Returns the raw string output of the tool or an error.
/// Does NOT interact with Conversation or OpContext directly.
pub async fn execute_tool_task_logic(
    tool_call: ToolCall,
    tool_executor: Arc<ToolExecutor>,
    // Remove Conversation and event_sender - they are handled in the main loop
    // conversation: Arc<Mutex<Conversation>>,
    // event_sender: Option<Sender<AppEvent>>,
    token: CancellationToken,
) -> std::result::Result<ToolResult, conductor_tools::ToolError> {
    // Return Result<String, ToolError> instead of TaskResult
    let tool_id = tool_call.id.clone();
    let tool_name = tool_call.name.clone();

    debug!(
        target: "app.context_util.execute_tool_task_logic",
        "Executing tool {} (ID: {}) logic", tool_name, tool_id,
    );

    // Check for cancellation before starting
    if token.is_cancelled() {
        // Return ToolError::Cancelled
        return Err(conductor_tools::ToolError::Cancelled(format!(
            "{tool_name} ({tool_id})"
        )));
    }

    // Use the cancellation-aware tool execution method
    let result = tool_executor
        .execute_tool_with_cancellation(&tool_call, token)
        .await;

    // Log outcome
    match &result {
        Ok(output) => debug!(
            target:"app.context_util.execute_tool_task_logic",
            "Tool {} ({}) completed successfully. Output type: {:?}",
                tool_name,
                tool_id,
                std::mem::discriminant(output),
        ),
        Err(e) => warn!(
            target:"app.context_util.execute_tool_task_logic",
            "Tool {} ({}) failed: {}", tool_name, tool_id, e,
        ),
    }

    // Return the raw Result<ToolResult, ToolError>
    result
}
