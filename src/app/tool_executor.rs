use anyhow::Result;
use std::collections::HashMap;

use crate::api::ToolCall;

/// Manages the execution of tools called by Claude
pub struct ToolExecutor {
    // We can add more state here if needed in the future
}

impl ToolExecutor {
    /// Create a new tool executor
    pub fn new() -> Self {
        Self {}
    }

    /// Execute a tool call and return the result
    pub async fn execute_tool(&self, tool_call: &ToolCall) -> Result<String> {
        crate::tools::execute_tool(&tool_call.name, &tool_call.parameters).await
    }

    /// Execute multiple tool calls in parallel and return their results
    pub async fn execute_tools(&self, tool_calls: Vec<ToolCall>) -> HashMap<String, Result<String>> {
        use futures::future::join_all;
        
        // Create a future for each tool call
        let futures = tool_calls.iter().map(|call| {
            let call_id = call.id.clone().unwrap_or_else(|| call.name.clone());
            let call = call.clone();
            
            async move {
                let result = self.execute_tool(&call).await;
                (call_id, result)
            }
        });
        
        // Execute all futures in parallel
        let results = join_all(futures).await;
        
        // Collect results into a HashMap
        results.into_iter().collect()
    }
}