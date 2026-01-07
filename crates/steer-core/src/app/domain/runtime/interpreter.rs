use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::api::provider::StreamChunk;
use crate::app::conversation::{AssistantContent, Message};
use crate::app::domain::delta::{StreamDelta, ToolCallDelta};
use crate::app::domain::types::{MessageId, OpId, SessionId, ToolCallId};
use crate::config::model::ModelId;
use crate::tools::ToolExecutor;
use steer_tools::{ToolCall, ToolError, ToolResult, ToolSchema};

#[derive(Clone)]
pub struct EffectInterpreter {
    api_client: Arc<ApiClient>,
    tool_executor: Arc<ToolExecutor>,
    session_id: Option<SessionId>,
}

impl EffectInterpreter {
    pub fn new(api_client: Arc<ApiClient>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            api_client,
            tool_executor,
            session_id: None,
        }
    }

    pub fn with_session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub async fn call_model(
        &self,
        model: ModelId,
        messages: Vec<Message>,
        system_prompt: Option<String>,
        tools: Vec<ToolSchema>,
        cancel_token: CancellationToken,
    ) -> Result<Vec<AssistantContent>, String> {
        self.call_model_with_deltas(
            model,
            messages,
            system_prompt,
            tools,
            cancel_token,
            None,
            None,
        )
        .await
    }

    pub async fn call_model_with_deltas(
        &self,
        model: ModelId,
        messages: Vec<Message>,
        system_prompt: Option<String>,
        tools: Vec<ToolSchema>,
        cancel_token: CancellationToken,
        delta_tx: Option<mpsc::Sender<StreamDelta>>,
        delta_context: Option<(OpId, MessageId)>,
    ) -> Result<Vec<AssistantContent>, String> {
        let tools_option = if tools.is_empty() { None } else { Some(tools) };

        let mut stream = self
            .api_client
            .stream_complete(
                &model,
                messages,
                system_prompt,
                tools_option,
                None,
                cancel_token,
            )
            .await
            .map_err(|e| e.to_string())?;

        let mut final_content: Option<Vec<AssistantContent>> = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                StreamChunk::TextDelta(text) => {
                    if let (Some(tx), Some((op_id, message_id))) = (&delta_tx, &delta_context) {
                        let delta = StreamDelta::TextChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            delta: text,
                        };
                        let _ = tx.send(delta).await;
                    }
                }
                StreamChunk::ThinkingDelta(thinking) => {
                    if let (Some(tx), Some((op_id, message_id))) = (&delta_tx, &delta_context) {
                        let delta = StreamDelta::ThinkingChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            delta: thinking,
                        };
                        let _ = tx.send(delta).await;
                    }
                }
                StreamChunk::ToolUseInputDelta { id, delta } => {
                    if let (Some(tx), Some((op_id, message_id))) = (&delta_tx, &delta_context) {
                        let delta = StreamDelta::ToolCallChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            tool_call_id: ToolCallId::from_string(&id),
                            delta: ToolCallDelta::ArgumentChunk(delta),
                        };
                        let _ = tx.send(delta).await;
                    }
                }
                StreamChunk::MessageComplete(response) => {
                    final_content = Some(response.content);
                }
                StreamChunk::Error(err) => {
                    return Err(err.to_string());
                }
                StreamChunk::ToolUseStart { .. } | StreamChunk::ContentBlockStop { .. } => {}
            }
        }

        final_content.ok_or_else(|| "Stream ended without MessageComplete".to_string())
    }

    pub async fn execute_tool(
        &self,
        tool_call: ToolCall,
        cancel_token: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        if let Some(session_id) = self.session_id {
            self.tool_executor
                .execute_tool_with_session(&tool_call, session_id, cancel_token)
                .await
        } else {
            self.tool_executor
                .execute_tool_with_cancellation(&tool_call, cancel_token)
                .await
        }
    }
}
