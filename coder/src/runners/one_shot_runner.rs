use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tools::{ToolCall, ToolResult, ToolSchema};
use tracing::{debug, info, warn};

use crate::api::{Client as ApiClient, Model, messages::Message};
use crate::app::{
    AgentEvent, AgentExecutor, AgentExecutorError, AgentExecutorRunRequest, ToolExecutor,
};
use crate::config::LlmConfig;
use crate::tools::{BackendRegistry, LocalBackend};

/// Contains the result of a single agent run, including the final assistant message
/// and all tool results produced during the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOnceResult {
    /// The final assistant message after all tools have been executed
    pub final_msg: Message,
    /// All tool results produced during the run (for audit logging)
    pub tool_results: Vec<ToolResult>,
}

/// Orchestrates a single non-interactive agent loop execution.
pub struct OneShotRunner {
    api_client: Arc<ApiClient>,
    tool_executor: ToolExecutor,
}

impl OneShotRunner {
    /// Creates a new OneShotRunner with the given LLM configuration
    pub fn new(cfg: &LlmConfig) -> Self {
        let api_client = Arc::new(ApiClient::new(cfg));

        let mut backend_registry = BackendRegistry::new();
        backend_registry.register("local".to_string(), Arc::new(LocalBackend::standard()));

        let tool_executor = ToolExecutor::new(Arc::new(backend_registry));

        Self {
            api_client,
            tool_executor,
        }
    }

    /// Runs the agent once and collects the final result
    ///
    /// * `init_msgs` - Initial messages to seed the conversation
    /// * `model` - Which LLM to use
    /// * `timeout` - Optional timeout for the entire operation
    pub async fn run(
        &self,
        init_msgs: Vec<Message>,
        model: Model,
        system_prompt: Option<String>,
        timeout: Option<Duration>,
    ) -> Result<RunOnceResult> {
        // 1. Create cancellation token with optional timeout
        let token = CancellationToken::new();

        if let Some(duration) = timeout {
            let timeout_token = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                timeout_token.cancel();
                info!("Timeout reached, cancellation token triggered");
            });
        }

        // 2. Create event channel and agent executor
        let (event_tx, mut event_rx) = mpsc::channel(100);
        let agent_executor = AgentExecutor::new(self.api_client.clone());
        let available_tools = self.tool_executor.to_api_tools();

        // 3. Create tool execution callback that automatically approves every tool
        let tool_executor = self.tool_executor.clone();
        let tool_callback = move |tool_call: ToolCall, token: CancellationToken| {
            let tool_executor = tool_executor.clone();
            async move {
                info!("Auto-approving tool in headless mode: {}", tool_call.name);
                // Execute the tool directly without asking for approval
                match tool_executor
                    .execute_tool_with_cancellation(&tool_call, token)
                    .await
                {
                    Ok(output) => Ok(output),
                    Err(e) => {
                        warn!("Tool execution error: {}", e);
                        Err(e)
                    }
                }
            }
        };

        // 4. Run the agent executor
        let request = AgentExecutorRunRequest {
            model,
            initial_messages: init_msgs,
            system_prompt,
            available_tools,
            tool_executor_callback: tool_callback,
        };

        let final_message = match agent_executor.run(request, event_tx, token).await {
            Ok(msg) => msg,
            Err(AgentExecutorError::Cancelled) => return Err(anyhow!("Operation timed out")),
            Err(e) => return Err(anyhow!("Agent execution error: {}", e)),
        };

        // 5. Collect all tool results from the event channel
        // The channel will be closed by the AgentExecutor when it finishes,
        // so recv().await will eventually return None.
        let mut tool_results = Vec::new();
        while let Some(event) = event_rx.recv().await {
            if let AgentEvent::ToolResultReceived(result) = event {
                tool_results.push(result);
            }
        }

        debug!("Run completed with {} tool results", tool_results.len());

        // 6. Return the final result
        Ok(RunOnceResult {
            final_msg: final_message,
            tool_results,
        })
    }
}
