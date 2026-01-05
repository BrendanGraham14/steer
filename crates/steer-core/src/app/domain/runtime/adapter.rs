use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::agent_executor::{AgentEvent, AgentExecutor, AgentExecutorRunRequest};
use crate::app::conversation::Message;
use crate::app::domain::action::Action;
use crate::app::domain::types::{MessageId, OpId, RequestId, SessionId, ToolCallId};
use crate::config::model::ModelId;
use crate::tools::ToolExecutor;
use steer_tools::{ToolCall, ToolError, ToolSchema};

pub struct AgentExecutorAdapter {
    executor: AgentExecutor,
    tool_executor: Arc<ToolExecutor>,
}

impl AgentExecutorAdapter {
    pub fn new(api_client: Arc<ApiClient>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            executor: AgentExecutor::new(api_client),
            tool_executor,
        }
    }

    pub async fn run_agent_loop(
        &self,
        session_id: SessionId,
        op_id: OpId,
        model: ModelId,
        messages: Vec<Message>,
        system_prompt: Option<String>,
        tools: Vec<ToolSchema>,
        token: CancellationToken,
        action_tx: mpsc::Sender<Action>,
    ) -> Result<(), AgentAdapterError> {
        let (event_tx, mut event_rx) = mpsc::channel::<AgentEvent>(32);

        let tool_executor = self.tool_executor.clone();
        let action_tx_clone = action_tx.clone();
        let session_id_clone = session_id;
        let token_clone = token.clone();

        let approval_callback = move |tool_call: ToolCall| {
            let action_tx = action_tx_clone.clone();
            let session_id = session_id_clone;

            async move {
                let request_id = RequestId::new();

                let action = Action::ToolApprovalRequested {
                    session_id,
                    request_id,
                    tool_call: tool_call.clone(),
                };

                action_tx.send(action).await.map_err(|_| {
                    ToolError::InternalError("Failed to send approval request".into())
                })?;

                Ok(crate::app::agent_executor::ApprovalDecision::Approved)
            }
        };

        let execution_callback = move |tool_call: ToolCall, cancel_token: CancellationToken| {
            let executor = tool_executor.clone();
            async move {
                executor
                    .execute_tool_with_cancellation(&tool_call, cancel_token)
                    .await
            }
        };

        let request = AgentExecutorRunRequest {
            model,
            initial_messages: messages,
            system_prompt,
            available_tools: tools,
            tool_approval_callback: approval_callback,
            tool_execution_callback: execution_callback,
        };

        let executor = self.executor.clone();
        let run_handle =
            tokio::spawn(async move { executor.run(request, event_tx, token_clone).await });

        while let Some(event) = event_rx.recv().await {
            let action = convert_agent_event_to_action(session_id, op_id, event);
            if action_tx.send(action).await.is_err() {
                break;
            }
        }

        match run_handle.await {
            Ok(Ok(_message)) => Ok(()),
            Ok(Err(e)) => Err(AgentAdapterError::Executor(e.to_string())),
            Err(e) => Err(AgentAdapterError::Task(e.to_string())),
        }
    }
}

fn convert_agent_event_to_action(session_id: SessionId, op_id: OpId, event: AgentEvent) -> Action {
    match event {
        AgentEvent::MessageFinal(message) => {
            let message_id = MessageId::from_string(message.id());
            let timestamp = current_timestamp();

            match &message.data {
                crate::app::conversation::MessageData::Assistant { content } => {
                    Action::ModelResponseComplete {
                        session_id,
                        op_id,
                        message_id,
                        content: content.clone(),
                        timestamp,
                    }
                }
                crate::app::conversation::MessageData::Tool {
                    tool_use_id,
                    result,
                } => {
                    let tool_call_id = ToolCallId::from_string(tool_use_id);
                    Action::ToolResult {
                        session_id,
                        tool_call_id,
                        tool_name: String::new(),
                        result: match result {
                            steer_tools::ToolResult::Error(e) => Err(e.clone()),
                            other => Ok(other.clone()),
                        },
                    }
                }
                _ => Action::Shutdown,
            }
        }
        AgentEvent::ExecutingTool {
            tool_call_id,
            name,
            parameters,
        } => {
            let tool_call_id = ToolCallId::from_string(tool_call_id);
            Action::ToolExecutionStarted {
                session_id,
                tool_call_id,
                tool_name: name.to_string(),
                tool_parameters: parameters.clone(),
            }
        }
    }
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, thiserror::Error)]
pub enum AgentAdapterError {
    #[error("Executor error: {0}")]
    Executor(String),

    #[error("Task error: {0}")]
    Task(String),

    #[error("Channel closed")]
    ChannelClosed,
}
