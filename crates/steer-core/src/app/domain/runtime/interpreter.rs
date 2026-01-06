use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::conversation::{AssistantContent, Message};
use crate::app::domain::types::SessionId;
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
        let tools_option = if tools.is_empty() { None } else { Some(tools) };

        let result = self
            .api_client
            .complete_with_retry(
                &model,
                &messages,
                &system_prompt,
                &tools_option,
                cancel_token,
                3,
            )
            .await;

        match result {
            Ok(response) => Ok(response.content),
            Err(e) => Err(e.to_string()),
        }
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
