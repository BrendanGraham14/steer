use anyhow::Result;
use std::collections::HashMap;
// use std::sync::Arc; // Removed unused import
// use tokio::sync::Mutex; // Removed unused import
use tokio_util::sync::CancellationToken;

use crate::api::ToolCall;

/// Manages the execution of tools called by Claude
#[derive(Clone)]
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
        crate::tools::execute_tool(&tool_call.name, &tool_call.parameters, None).await
    }

    /// Execute a tool call with cancellation support and return the result
    pub async fn execute_tool_with_cancellation(
        &self,
        tool_call: &ToolCall,
        token: CancellationToken,
    ) -> Result<String> {
        crate::tools::execute_tool(&tool_call.name, &tool_call.parameters, Some(token)).await
    }

    /// Execute multiple tool calls in parallel and return their results
    pub async fn execute_tools(
        &self,
        tool_calls: Vec<ToolCall>,
    ) -> HashMap<String, Result<String>> {
        use futures::future::join_all;

        // Create a future for each tool call
        let futures = tool_calls.iter().map(|call| {
            let call_id = call.id.clone();
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
