use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::api::error::{ApiError, StreamError};
use crate::app::SystemContext;
use crate::app::conversation::{AssistantContent, Message};
use crate::auth::{AuthStorage, DynAuthenticationFlow};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::{ToolCall, ToolSchema};

#[derive(Debug, Clone)]
pub enum StreamChunk {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStart { id: String, name: String },
    ToolUseInputDelta { id: String, delta: String },
    ContentBlockStop { index: usize },
    MessageComplete(CompletionResponse),
    Error(StreamError),
}

pub type CompletionStream = Pin<Box<dyn Stream<Item = StreamChunk> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletionResponse {
    pub content: Vec<AssistantContent>,
}

impl CompletionResponse {
    /// Extract all text content from the response
    pub fn extract_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let AssistantContent::Text { text } = block {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<String>()
    }

    /// Check if the response contains any tool calls
    pub fn has_tool_calls(&self) -> bool {
        self.content
            .iter()
            .any(|block| matches!(block, AssistantContent::ToolCall { .. }))
    }

    pub fn extract_tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|block| {
                if let AssistantContent::ToolCall { tool_call, .. } = block {
                    Some(tool_call.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

#[async_trait]
pub trait Provider: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError>;

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let response = self
            .complete(model_id, messages, system, tools, call_options, token)
            .await?;
        Ok(Box::pin(futures_util::stream::once(async move {
            StreamChunk::MessageComplete(response)
        })))
    }

    fn create_auth_flow(
        &self,
        _storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>> {
        None
    }
}
