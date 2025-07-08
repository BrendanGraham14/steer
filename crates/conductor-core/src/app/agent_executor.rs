use crate::api::{ApiError, Client as ApiClient, Model};
use crate::app::conversation::Message;
use conductor_tools::{ToolCall, ToolError, ToolSchema, result::ToolResult as ConductorToolResult};
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[derive(Debug)]
pub enum AgentEvent {
    AssistantMessagePart(String),
    AssistantMessageFinal(Message),
    ExecutingTool {
        tool_call_id: String,
        name: String,
    },
    ToolResultReceived {
        tool_call_id: String,
        message_id: String,
        result: ConductorToolResult,
    },
}

#[derive(Error, Debug)]
pub enum AgentExecutorError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),
    #[error("Tool execution error: {0}")]
    Tool(#[from] ToolError),
    #[error("Event channel send error: {0}")]
    SendError(String),
    #[error("Operation cancelled")]
    Cancelled,
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Unexpected API response structure")]
    UnexpectedResponse,
}

impl<T> From<mpsc::error::SendError<T>> for AgentExecutorError {
    fn from(err: mpsc::error::SendError<T>) -> Self {
        AgentExecutorError::SendError(err.to_string())
    }
}

#[derive(Clone)]
pub struct AgentExecutor {
    api_client: Arc<ApiClient>,
}

pub struct AgentExecutorRunRequest<F> {
    pub model: Model,
    pub initial_messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub available_tools: Vec<ToolSchema>,
    pub tool_executor_callback: F,
}

impl AgentExecutor {
    pub fn new(api_client: Arc<ApiClient>) -> Self {
        Self { api_client }
    }

    #[instrument(skip_all, name = "AgentExecutor::run")]
    pub async fn run<F, Fut>(
        &self,
        request: AgentExecutorRunRequest<F>,
        event_sender: mpsc::Sender<AgentEvent>,
        token: CancellationToken,
    ) -> Result<Message, AgentExecutorError>
    where
        F: Fn(ToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ConductorToolResult, ToolError>> + Send + 'static,
    {
        let mut messages = request.initial_messages.clone();
        let tools = if request.available_tools.is_empty() {
            None
        } else {
            Some(request.available_tools)
        };

        debug!(target: "AgentExecutor::run", "About to start completion loop with model: {:?}", request.model);

        loop {
            if token.is_cancelled() {
                info!("Operation cancelled before API call.");
                return Err(AgentExecutorError::Cancelled);
            }

            info!(target: "AgentExecutor::run", model = ?request.model, "Calling LLM API");
            let completion_response = self
                .api_client
                .complete_with_retry(
                    request.model,
                    &messages,
                    &request.system_prompt,
                    &tools,
                    token.clone(),
                    3,
                )
                .await?;
            let tool_calls = completion_response.extract_tool_calls();

            // Get thread info from the last message
            let (thread_id, parent_id) = if let Some(last_msg) = messages.last() {
                (*last_msg.thread_id(), last_msg.id().to_string())
            } else {
                // This shouldn't happen
                return Err(AgentExecutorError::Internal(
                    "No messages in conversation when adding assistant message".to_string(),
                ));
            };

            let full_assistant_message = Message::Assistant {
                content: completion_response.content,
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                id: Uuid::new_v4().to_string(),
                thread_id,
                parent_message_id: Some(parent_id),
            };

            messages.push(full_assistant_message.clone());

            if tool_calls.is_empty() {
                info!("LLM response received, no tool calls requested.");
                debug!(target: "AgentExecutor::run_operation", "Sending AssistantMessageFinal event (no tool calls).");
                event_sender
                    .send(AgentEvent::AssistantMessageFinal(
                        full_assistant_message.clone(),
                    ))
                    .await?;
                debug!(target: "AgentExecutor::run_operation", "Operation finished successfully (no tool calls), returning final message.");
                return Ok(full_assistant_message);
            } else {
                info!(count = tool_calls.len(), "LLM requested tool calls.");
                debug!(target: "AgentExecutor::run_operation", "Sending AssistantMessageFinal event (with tool calls).");
                event_sender
                    .send(AgentEvent::AssistantMessageFinal(
                        full_assistant_message.clone(),
                    ))
                    .await?;

                let tool_results_with_ids = self
                    .handle_tool_calls(
                        &tool_calls,
                        &request.tool_executor_callback,
                        &event_sender,
                        &token,
                    )
                    .await?;

                if token.is_cancelled() {
                    info!("Operation cancelled during or after tool handling.");
                    return Err(AgentExecutorError::Cancelled);
                }

                // Add tool results to messages - one message per tool result
                for (i, (tool_result, message_id)) in tool_results_with_ids.iter().enumerate() {
                    // Get thread info from the last message
                    let (thread_id, parent_id) = if let Some(last_msg) = messages.last() {
                        (*last_msg.thread_id(), last_msg.id().to_string())
                    } else {
                        return Err(AgentExecutorError::Internal(
                            "No messages in conversation when adding tool results".to_string(),
                        ));
                    };

                    messages.push(Message::Tool {
                        tool_use_id: tool_calls[i].id.clone(),
                        result: tool_result.clone(),
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                        id: message_id.clone(),
                        thread_id,
                        parent_message_id: Some(parent_id),
                    });
                }

                debug!("Looping back to LLM with tool results.");
            }
        }
    }

    #[instrument(
        skip(self, tool_calls, tool_executor_callback, event_sender, token),
        name = "AgentExecutor::handle_tool_calls"
    )]
    async fn handle_tool_calls<F, Fut>(
        &self,
        tool_calls: &[ToolCall],
        tool_executor_callback: &F,
        event_sender: &mpsc::Sender<AgentEvent>,
        token: &CancellationToken,
    ) -> Result<Vec<(ConductorToolResult, String)>, AgentExecutorError>
    where
        F: Fn(ToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ConductorToolResult, ToolError>> + Send + 'static,
    {
        info!("Processing tool calls via provided callback.");
        let futures: Vec<_> = tool_calls
                    .iter()
                    .map(|call| {
                        // Clone necessary items for the async block
                        let call_id = call.id.clone();
                        let tool_name = call.name.clone(); // Keep for potential cancellation error
                        let event_sender_clone = event_sender.clone();
                        let token_clone = token.clone();

                        // The callback is now responsible for both tool approval and execution
                        async move {
                            // Invoke the callback which handles approval + execution
                            // The callback now returns either:
                            // 1. Ok(output) - Tool was approved and executed successfully
                            // 2. Err(ToolError::DeniedByUser) - Tool was denied by user
                            // 3. Err(other) - Tool was approved but failed for other reasons

                            // Call the callback - this handles approval logic inside
                            let result = tokio::select! {
                                biased;
                                _ = token_clone.cancelled() => {
                                    warn!(tool_id=%call_id, tool_name=%tool_name, "Cancellation detected during tool callback/approval for tool");
                                    Err(ToolError::Cancelled(tool_name.clone())) // Return cancellation error
                               }
                                res = tool_executor_callback(call.clone(), token_clone.clone()) => res,
                            };

                            let message_id = uuid::Uuid::new_v4().to_string();

                            let tool_result = match result {
                                Ok(output) => {
                                    debug!(tool_id=%call_id, tool_name=%tool_name, "Tool executed successfully via callback");

                                    // Send ExecutingTool event for successful execution
                                    if let Err(e) = event_sender_clone
                                        .send(AgentEvent::ExecutingTool {
                                            tool_call_id: call_id.clone(),
                                            name: tool_name.clone(),
                                        })
                                        .await
                                    {
                                        warn!(tool_id=%call_id, tool_name=%tool_name, "Failed to send ExecutingTool event: {}", e);
                                    }

                                    output
                                }
                                Err(e @ ToolError::DeniedByUser(_)) => {
                                    // Tool was denied, don't send ExecutingTool event
                                    warn!(tool_id=%call_id, tool_name=%tool_name, "Tool callback resulted in denial");
                                    ConductorToolResult::Error(e)
                                }
                                Err(e @ ToolError::Cancelled(_)) => {
                                    // Tool was cancelled
                                    warn!(tool_id=%call_id, tool_name=%tool_name, "Tool callback resulted in cancellation: {}", e);
                                    ConductorToolResult::Error(e)
                                }
                                Err(e) => {
                                    // Other errors (tool was approved but failed during execution)
                                    error!(tool_id=%call_id, tool_name=%tool_name, "Tool callback failed: {}", e);

                                    // Still send ExecutingTool event since the tool was attempted
                                    if let Err(send_err) = event_sender_clone
                                        .send(AgentEvent::ExecutingTool {
                                            tool_call_id: call_id.clone(),
                                            name: tool_name.clone(),
                                        })
                                        .await
                                    {
                                        warn!(tool_id=%call_id, tool_name=%tool_name, "Failed to send ExecutingTool event: {}", send_err);
                                    }

                                    ConductorToolResult::Error(e)
                                }
                            };

                            // Send the final result event (success, denied, cancelled, or other error)
                            if let Err(e) = event_sender_clone
                                .send(AgentEvent::ToolResultReceived {
                                    tool_call_id: call_id.clone(),
                                    message_id: message_id.clone(),
                                    result: tool_result.clone(),
                                })
                                .await
                            {
                                error!("Failed to send ToolResultReceived event: {}", e);
                            }
                            (tool_result, message_id)
                        }
                    })
                    .collect();

        Ok(futures::future::join_all(futures).await)
    }
}
