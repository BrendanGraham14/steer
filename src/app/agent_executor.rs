use crate::api::{
    ApiError, Client as ApiClient, Model,
    messages::ContentBlock,
    messages::{Message, MessageContent, MessageRole, StructuredContent},
    tools::{Tool, ToolCall, ToolResult},
};
use crate::tools::ToolError;
use std::future::Future;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, instrument, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[derive(Debug)]
pub enum AgentEvent {
    AssistantMessagePart(String),
    AssistantMessageFinal(Message),
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
    #[error("Operation cancelled")]
    Cancelled,
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Unexpected API response structure")]
    UnexpectedResponse,
}

impl AgentExecutorError {
    pub fn into_anyhow_error(self) -> anyhow::Error {
        anyhow::Error::msg(self.to_string())
    }
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
    pub available_tools: Vec<Tool>,
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
        Fut: Future<Output = Result<String, ToolError>> + Send + 'static,
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
                .complete(
                    request.model,
                    messages.clone(), // Clone messages for the API call
                    request.system_prompt.clone(),
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

            let full_assistant_message = Message {
                id: None,
                role: MessageRole::Assistant,
                content: MessageContent::StructuredContent {
                    content: StructuredContent(content_blocks),
                },
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

                let tool_results = self
                    .handle_tool_calls(
                        tool_calls,
                        &request.tool_executor_callback,
                        &event_sender,
                        &token,
                    )
                    .await?;

                if token.is_cancelled() {
                    info!("Operation cancelled during or after tool handling.");
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
        }
    }

    #[instrument(
        skip(self, tool_calls, tool_executor_callback, event_sender, token),
        name = "AgentExecutor::handle_tool_calls"
    )]
    async fn handle_tool_calls<F, Fut>(
        &self,
        tool_calls: Vec<ToolCall>,
        tool_executor_callback: &F,
        event_sender: &mpsc::Sender<AgentEvent>,
        token: &CancellationToken,
    ) -> Result<Vec<ToolResult>, AgentExecutorError>
    where
        F: Fn(ToolCall, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, ToolError>> + Send + 'static,
    {
        info!("Processing tool calls via provided callback.");
        let futures: Vec<_> = tool_calls
                    .into_iter()
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

                            // Create the ToolResult based on the callback's outcome
                            let tool_result = match &result {
                                Ok(output) => {
                                    // Tool was approved and executed successfully
                                    debug!(tool_id=%call_id, tool_name=%tool_name, "Tool executed successfully via callback");

                                    // Send the ExecutingTool event (tool was successfully executed)
                                    if let Err(e) = event_sender_clone
                                        .send(AgentEvent::ExecutingTool {
                                            tool_call_id: call_id.clone(),
                                            name: tool_name.clone(),
                                        })
                                        .await
                                    {
                                        warn!(tool_id=%call_id, tool_name=%tool_name, "Failed to send ExecutingTool event: {}", e);
                                    }

                                    ToolResult::success(call_id.clone(), output.clone())
                                }
                                Err(ToolError::DeniedByUser(_)) => {
                                    // Tool was denied, don't send ExecutingTool event
                                    warn!(tool_id=%call_id, tool_name=%tool_name, "Tool callback resulted in denial");
                                    ToolResult::error(call_id.clone(), format!("Tool execution denied by user: {}", tool_name))
                                },
                                Err(e @ ToolError::Cancelled(_)) => {
                                    // Propagate cancellation error specifically
                                    warn!(tool_id=%call_id, tool_name=%tool_name, "Tool callback resulted in cancellation: {}", e);
                                    ToolResult::error(call_id.clone(), e.to_string()) // Report as error
                                },
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

                                    ToolResult::error(call_id.clone(), e.to_string()) // Report other errors
                                }
                            };

                            // Send the final result event (success, denied, cancelled, or other error)
                            if let Err(e) = event_sender_clone
                                .send(AgentEvent::ToolResultReceived(tool_result.clone()))
                                .await
                            {
                                error!("Failed to send ToolResultReceived event: {}", e);
                                // Don't lose the original result if send fails
                            }
                            tool_result // Return the result for collection by join_all
                        }
                    })
                    .collect();

        Ok(futures::future::join_all(futures).await)
    }
}
