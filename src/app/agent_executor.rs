use crate::api::{
    ApiError, Client as ApiClient, Model,
    messages::ContentBlock,
    messages::{Message, MessageContent, MessageRole, StructuredContent},
    tools::{Tool, ToolCall, ToolResult},
};
use crate::tools::ToolError;
use std::{future::Future, sync::Arc};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    Automatic,
    Interactive,
}

// Moved ApprovalDecision here to be public within the module
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

// New struct for individual tool approval requests
#[derive(Debug)]
pub struct ToolApprovalRequest {
    pub index: usize, // Original index for potential ordering needs
    pub tool_call: ToolCall,
    pub responder: oneshot::Sender<ApprovalDecision>,
}

#[derive(Debug)]
pub enum AgentEvent {
    AssistantMessagePart(String),
    AssistantMessageFinal(Message),
    RequestToolApprovals(Vec<ToolApprovalRequest>), // Updated variant
    ExecutingTool { tool_call_id: String, name: String },
    ToolResultReceived(ToolResult),
}

#[derive(Error, Debug)]
pub enum AgentExecutorError {
    #[error("API error: {0}")]
    Api(#[from] ApiError),
    #[error("Tool execution error: {0}")]
    Tool(#[from] ToolError),
    #[error("Event channel send error: {0}")]
    SendError(String),
    #[error("Approval channel receive error")]
    ApprovalRecvError(#[from] oneshot::error::RecvError), // Updated for oneshot
    #[error("Operation cancelled")]
    Cancelled,
    #[error("No tool calls provided for interactive approval")]
    NoToolCallsForApproval,
    #[error("Tool approval response channel closed unexpectedly")]
    ApprovalResponseChannelClosed,
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Unexpected API response structure")]
    UnexpectedResponse,
}

// Helper method to convert AgentExecutorError to anyhow::Error
impl AgentExecutorError {
    pub fn into_anyhow_error(self) -> anyhow::Error {
        anyhow::Error::msg(self.to_string())
    }
}

// Helper to convert SendError into AgentExecutorError
impl<T> From<mpsc::error::SendError<T>> for AgentExecutorError {
    fn from(err: mpsc::error::SendError<T>) -> Self {
        AgentExecutorError::SendError(err.to_string())
    }
}

#[derive(Clone)] // Added Clone
pub struct AgentExecutor {
    api_client: Arc<ApiClient>,
}

impl AgentExecutor {
    pub fn new(api_client: Arc<ApiClient>) -> Self {
        // Added Send + Sync
        Self { api_client }
    }

    #[instrument(skip_all, name = "AgentExecutor::run_operation")]
    #[allow(clippy::too_many_arguments)] // Necessary complexity for this function
    pub async fn run_operation<F, Fut>(
        &self,
        model: Model,
        initial_messages: Vec<Message>,
        system_prompt: Option<String>,
        available_tools: Vec<Tool>,
        tool_executor_callback: F,
        event_sender: mpsc::Sender<AgentEvent>,
        approval_mode: ApprovalMode,
        token: CancellationToken,
    ) -> Result<Message, AgentExecutorError>
    where
        F: Fn(ToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, ToolError>> + Send + 'static,
    {
        let mut messages = initial_messages.clone();
        let tools = if available_tools.is_empty() {
            None
        } else {
            Some(available_tools)
        };

        debug!(target: "AgentExecutor::run_operation", "About to start completion loop with model: {:?}", model);

        loop {
            if token.is_cancelled() {
                info!("Operation cancelled before API call.");
                return Err(AgentExecutorError::Cancelled);
            }

            info!(target: "AgentExecutor::run_operation", model = ?model, "Calling LLM API");
            let completion_response = self
                .api_client
                .complete(
                    model,
                    messages.clone(), // Clone messages for the API call
                    system_prompt.clone(),
                    tools.clone(),
                    token.clone(),
                )
                .await?;

            // Create structured content to collect all blocks
            let mut content_blocks = Vec::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            for block in completion_response.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        content_blocks.push(crate::api::messages::ContentBlock::Text { text });
                    }
                    ContentBlock::ToolUse {
                        id, name, input, ..
                    } => {
                        let tool_call = ToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            parameters: input.clone(),
                        };
                        debug!(tool_id=%id, tool_name=%name, "Complete tool call received");
                        content_blocks.push(crate::api::messages::ContentBlock::ToolUse {
                            id,
                            name,
                            input,
                        });
                        tool_calls.push(tool_call);
                        // We need to reconstruct the ToolCalls content block later if tools were used
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        content_blocks.push(crate::api::messages::ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content,
                            is_error,
                        });
                    }
                }
            }

            // Create the full assistant message
            let full_assistant_message = Message {
                id: None,
                role: MessageRole::Assistant,
                content: MessageContent::StructuredContent {
                    content: StructuredContent(content_blocks),
                },
            };

            // Add the complete assistant message to history *before* tool processing
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

                // --- Handle Tool Calls ---
                let tool_results = self
                    .handle_tool_calls(
                        tool_calls,
                        &tool_executor_callback,
                        &event_sender,
                        approval_mode,
                        &token,
                    )
                    .await?;

                if token.is_cancelled() {
                    return Err(AgentExecutorError::Cancelled);
                }

                // Convert tool results to content blocks
                let tool_result_blocks = tool_results
                    .iter()
                    .map(|tr| crate::api::messages::ContentBlock::ToolResult {
                        tool_use_id: tr.tool_call_id.clone(),
                        content: vec![crate::api::messages::ContentBlock::Text {
                            text: tr.output.clone(),
                        }],
                        is_error: if tr.is_error { Some(true) } else { None },
                    })
                    .collect::<Vec<_>>();

                // Add tool results to messages
                messages.push(Message {
                    id: None,
                    role: MessageRole::Tool,
                    content: MessageContent::StructuredContent {
                        content: StructuredContent(tool_result_blocks),
                    },
                });

                debug!("Looping back to LLM with tool results.");
            }
        } // End main loop
    }

    #[instrument(
        skip(self, tool_calls, tool_executor_callback, event_sender, token),
        name = "AgentExecutor::handle_tool_calls"
    )]
    async fn handle_tool_calls<F, Fut>(
        &self,
        tool_calls: Vec<ToolCall>,
        tool_executor_callback: &F, // Borrow the callback closure
        event_sender: &mpsc::Sender<AgentEvent>, // Borrow the sender
        approval_mode: ApprovalMode,
        token: &CancellationToken, // Borrow the token
    ) -> Result<Vec<ToolResult>, AgentExecutorError>
    where
        F: Fn(ToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, ToolError>> + Send + 'static,
    {
        let mut results = Vec::with_capacity(tool_calls.len());

        match approval_mode {
            ApprovalMode::Automatic => {
                info!("Executing tools automatically.");
                let futures: Vec<_> = tool_calls
                    .into_iter()
                    .map(|call| {
                        let call_id = call.id.clone();
                        let tool_name = call.name.clone();
                        let event_sender_clone = event_sender.clone();
                        let token_clone = token.clone();

                        async move {
                            // Send executing tool event and log any errors
                            if let Err(e) = event_sender_clone
                                .send(AgentEvent::ExecutingTool {
                                    tool_call_id: call_id.clone(),
                                    name: tool_name.clone(),
                                })
                                .await
                            {
                                error!("Failed to send ExecutingTool event: {}", e);
                            }

                            let result = tokio::select! {
                                biased;
                                res = tool_executor_callback(call, token_clone.clone()) => res,
                                _ = token_clone.cancelled() => Err(ToolError::Cancelled(tool_name.clone())),
                            };

                            let tool_result = match result {
                                Ok(output) => ToolResult::success(call_id.clone(), output),
                                Err(e) => ToolResult::error(call_id.clone(), e.to_string()),
                            };
                            // Ensure the event is sent and propagate any errors
                            if let Err(e) = event_sender_clone
                                .send(AgentEvent::ToolResultReceived(tool_result.clone()))
                                .await
                            {
                                error!("Failed to send ToolResultReceived event: {}", e);
                            }
                            tool_result
                        }
                    })
                    .collect();

                results = futures::future::join_all(futures).await;
            }
            ApprovalMode::Interactive => {
                info!("Requesting interactive tool approval.");

                let mut pending_decisions = Vec::with_capacity(tool_calls.len());
                let mut approval_requests = Vec::with_capacity(tool_calls.len());

                for (index, tool_call) in tool_calls.iter().enumerate() {
                    let (tx, rx) = oneshot::channel();
                    approval_requests.push(ToolApprovalRequest {
                        index,
                        tool_call: tool_call.clone(),
                        responder: tx,
                    });
                    pending_decisions.push(rx);
                }

                // Send the batch of approval requests to the App actor
                event_sender
                    .send(AgentEvent::RequestToolApprovals(approval_requests))
                    .await?;

                // Wait for all decisions concurrently, respecting the overall cancellation token
                let decision_futs = pending_decisions.into_iter();

                let decision_results = tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        info!("Operation cancelled while waiting for tool approvals.");
                        return Err(AgentExecutorError::Cancelled);
                    }
                    res = futures::future::join_all(decision_futs) => res,
                };

                // Process decisions and execute approved tools
                let mut execution_futures = Vec::new();
                for (index, tool_call) in tool_calls.into_iter().enumerate() {
                    let call_id = tool_call.id.clone();
                    let tool_name = tool_call.name.clone();

                    let decision = match decision_results.get(index) {
                        Some(Ok(d)) => *d,
                        Some(Err(e)) => {
                            // oneshot::error::RecvError means the sender (App actor) was dropped.
                            // This could be due to cancellation, UI closing, or an error in the actor.
                            warn!(tool_id=%call_id, tool_name=%tool_name, "Approval channel closed for tool: {}", e);
                            ApprovalDecision::Denied // Treat as denied
                        }
                        None => {
                            // Should not happen if join_all lengths match
                            error!(tool_id=%call_id, tool_name=%tool_name, "Internal error: Missing decision for tool index {}", index);
                            ApprovalDecision::Denied
                        }
                    };

                    match decision {
                        ApprovalDecision::Approved => {
                            info!(tool_id=%call_id, tool_name=%tool_name, "Executing approved tool");
                            let event_sender_clone = event_sender.clone();
                            let token_clone = token.clone(); // Clone token for the execution future

                            execution_futures.push(async move {
                                if let Err(e) = event_sender_clone
                                    .send(AgentEvent::ExecutingTool {
                                        tool_call_id: call_id.clone(),
                                        name: tool_name.clone(),
                                    })
                                    .await
                                {
                                    error!("Failed to send ExecutingTool event: {}", e);
                                }

                                let result = tokio::select! {
                                    biased;
                                    res = tool_executor_callback(tool_call, token_clone.clone()) => res,
                                    _ = token_clone.cancelled() => Err(ToolError::Cancelled(tool_name.clone())),
                                };

                                let tool_result = match result {
                                    Ok(output) => ToolResult::success(call_id.clone(), output),
                                    Err(e) => ToolResult::error(call_id.clone(), e.to_string()),
                                };

                                if let Err(e) = event_sender_clone
                                    .send(AgentEvent::ToolResultReceived(tool_result.clone()))
                                    .await
                                {
                                    error!("Failed to send ToolResultReceived event: {}", e);
                                }
                                tool_result
                            });
                        }
                        ApprovalDecision::Denied => {
                            warn!(tool_id=%call_id, tool_name=%tool_name, "Skipping denied tool call");
                            let denied_result = ToolResult::error(
                                call_id.clone(),
                                "Tool execution denied by user.".to_string(),
                            );
                            // Send result event even for denied tools so UI can update status
                            if let Err(e) = event_sender
                                .send(AgentEvent::ToolResultReceived(denied_result.clone()))
                                .await
                            {
                                error!(
                                    "Failed to send ToolResultReceived event for denied tool: {}",
                                    e
                                );
                            }
                            results.push(denied_result); // Add to the final results list immediately
                        }
                    }
                }
                // Await all approved tool executions
                let executed_results = futures::future::join_all(execution_futures).await;
                results.extend(executed_results); // Add executed results to the list
            }
        }

        Ok(results)
    }
}
