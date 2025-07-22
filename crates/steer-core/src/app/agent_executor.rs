use crate::api::{ApiError, Client as ApiClient, Model};
use crate::app::conversation::{Message, MessageData};
use futures::{StreamExt, stream::FuturesUnordered};
use std::future::Future;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use steer_tools::{ToolCall, ToolError, ToolSchema, result::ToolResult as SteerToolResult};
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
    MessageFinal(Message),
    ExecutingTool {
        tool_call_id: String,
        name: String,
        parameters: serde_json::Value,
    },
}

#[derive(Error, Debug)]
pub enum AgentExecutorError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
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

pub struct AgentExecutorRunRequest<A, E> {
    pub model: Model,
    pub initial_messages: Vec<Message>,
    pub system_prompt: Option<String>,
    pub available_tools: Vec<ToolSchema>,
    pub tool_approval_callback: A,
    pub tool_execution_callback: E,
}

impl AgentExecutor {
    pub fn new(api_client: Arc<ApiClient>) -> Self {
        Self { api_client }
    }

    #[instrument(skip_all, name = "AgentExecutor::run")]
    pub async fn run<A, AFut, E, EFut>(
        &self,
        request: AgentExecutorRunRequest<A, E>,
        event_sender: mpsc::Sender<AgentEvent>,
        token: CancellationToken,
    ) -> Result<Message, AgentExecutorError>
    where
        A: Fn(ToolCall) -> AFut + Send + Sync + 'static,
        AFut: Future<Output = Result<ApprovalDecision, ToolError>> + Send + 'static,
        E: Fn(ToolCall, CancellationToken) -> EFut + Send + Sync + 'static,
        EFut: Future<Output = Result<SteerToolResult, ToolError>> + Send + 'static,
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

            // Get parent info from the last message
            let parent_id = if let Some(last_msg) = messages.last() {
                last_msg.id().to_string()
            } else {
                // This shouldn't happen
                return Err(AgentExecutorError::Internal(
                    "No messages in conversation when adding assistant message".to_string(),
                ));
            };

            let full_assistant_message = Message {
                data: MessageData::Assistant {
                    content: completion_response.content,
                },
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                id: Uuid::new_v4().to_string(),
                parent_message_id: Some(parent_id),
            };

            messages.push(full_assistant_message.clone());

            if tool_calls.is_empty() {
                info!("LLM response received, no tool calls requested.");
                event_sender
                    .send(AgentEvent::MessageFinal(full_assistant_message.clone()))
                    .await?;
                debug!(target: "AgentExecutor::run_operation", "Operation finished successfully (no tool calls), returning final message.");
                return Ok(full_assistant_message);
            } else {
                info!(count = tool_calls.len(), "LLM requested tool calls.");
                event_sender
                    .send(AgentEvent::MessageFinal(full_assistant_message.clone()))
                    .await?;

                // Create concurrent futures for every tool call
                let mut pending_tools: FuturesUnordered<_> = tool_calls
                    .into_iter()
                    .map(|call| {
                        let event_sender_clone = event_sender.clone();
                        let token_clone = token.clone();
                        let approval_callback = &request.tool_approval_callback;
                        let execution_callback = &request.tool_execution_callback;

                        async move {
                            let message_id = uuid::Uuid::new_v4().to_string();
                            let call_id = call.id.clone();

                            // Handle single tool call
                            let result = Self::handle_single_tool_call(
                                call,
                                approval_callback,
                                execution_callback,
                                &event_sender_clone,
                                token_clone,
                            )
                            .await;

                            (call_id, message_id, result)
                        }
                    })
                    .collect();

                // Pull results as they finish and emit events
                while let Some((tool_call_id, message_id, result)) = pending_tools.next().await {
                    if token.is_cancelled() {
                        info!("Operation cancelled during tool handling.");
                        return Err(AgentExecutorError::Cancelled);
                    }

                    // Get parent info from the last message
                    let parent_id = if let Some(last_msg) = messages.last() {
                        last_msg.id().to_string()
                    } else {
                        return Err(AgentExecutorError::Internal(
                            "No messages in conversation when adding tool results".to_string(),
                        ));
                    };

                    // Add tool result message
                    let tool_message = Message {
                        data: MessageData::Tool {
                            tool_use_id: tool_call_id,
                            result,
                        },
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs(),
                        id: message_id,
                        parent_message_id: Some(parent_id),
                    };

                    messages.push(tool_message.clone());
                    event_sender
                        .send(AgentEvent::MessageFinal(tool_message))
                        .await?;
                }

                debug!("Looping back to LLM with tool results.");
            }
        }
    }

    #[instrument(
        skip(tool_call, approval_callback, execution_callback, event_sender, token),
        name = "AgentExecutor::handle_single_tool_call"
    )]
    async fn handle_single_tool_call<A, AFut, E, EFut>(
        tool_call: ToolCall,
        approval_callback: &A,
        execution_callback: &E,
        event_sender: &mpsc::Sender<AgentEvent>,
        token: CancellationToken,
    ) -> SteerToolResult
    where
        A: Fn(ToolCall) -> AFut + Send + Sync + 'static,
        AFut: Future<Output = Result<ApprovalDecision, ToolError>> + Send + 'static,
        E: Fn(ToolCall, CancellationToken) -> EFut + Send + Sync + 'static,
        EFut: Future<Output = Result<SteerToolResult, ToolError>> + Send + 'static,
    {
        let call_id = tool_call.id.clone();
        let tool_name = tool_call.name.clone();

        // First, check approval
        let approval_result = tokio::select! {
            biased;
            _ = token.cancelled() => {
                warn!(tool_id=%call_id, tool_name=%tool_name, "Cancellation detected during tool approval");
                Err(ToolError::Cancelled(tool_name.clone()))
            }
            res = approval_callback(tool_call.clone()) => res,
        };

        match approval_result {
            Ok(ApprovalDecision::Approved) => {
                debug!(tool_id=%call_id, tool_name=%tool_name, "Tool approved, executing");

                // Send ExecutingTool event for approved execution
                if let Err(e) = event_sender
                    .send(AgentEvent::ExecutingTool {
                        tool_call_id: call_id.clone(),
                        name: tool_name.clone(),
                        parameters: tool_call.parameters.clone(),
                    })
                    .await
                {
                    warn!(tool_id=%call_id, tool_name=%tool_name, "Failed to send ExecutingTool event: {}", e);
                }

                // Execute the tool
                let execution_result = tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        warn!(tool_id=%call_id, tool_name=%tool_name, "Cancellation detected during tool execution");
                        Err(ToolError::Cancelled(tool_name.clone()))
                    }
                    res = execution_callback(tool_call, token.clone()) => res,
                };

                match execution_result {
                    Ok(output) => {
                        debug!(tool_id=%call_id, tool_name=%tool_name, "Tool executed successfully");
                        output
                    }
                    Err(e) => {
                        error!(tool_id=%call_id, tool_name=%tool_name, "Tool execution failed: {}", e);
                        SteerToolResult::Error(e)
                    }
                }
            }
            Ok(ApprovalDecision::Denied) => {
                warn!(tool_id=%call_id, tool_name=%tool_name, "Tool approval denied");
                SteerToolResult::Error(ToolError::DeniedByUser(tool_name))
            }
            Err(e @ ToolError::Cancelled(_)) => {
                warn!(tool_id=%call_id, tool_name=%tool_name, "Tool approval cancelled: {}", e);
                SteerToolResult::Error(e)
            }
            Err(e) => {
                error!(tool_id=%call_id, tool_name=%tool_name, "Tool approval failed: {}", e);
                SteerToolResult::Error(e)
            }
        }
    }
}
