use anyhow::Result;
use crate::api::ToolCall;

/// Handle a tool call from Claude
pub async fn handle_tool_call(tool_call: &ToolCall) -> Result<String> {
    // Execute the tool and return the result
    crate::tools::execute_tool(&tool_call.name, &tool_call.parameters).await
}