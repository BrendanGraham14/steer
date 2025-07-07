use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::api::error::ApiError;
use crate::app::conversation::{AssistantContent, Message};
use crate::auth::{AuthStorage, DynAuthenticationFlow};
use steer_tools::{ToolCall, ToolSchema};

use super::Model;

/// Response from the provider's completion API
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
            .collect::<Vec<String>>()
            .join("")
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
                if let AssistantContent::ToolCall { tool_call } = block {
                    Some(tool_call.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Provider trait that all LLM providers must implement
#[async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Get the name of the provider
    fn name(&self) -> &'static str;

    /// Complete a prompt with the LLM
    async fn complete(
        &self,
        model: Model,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError>;

    /// Create an authentication flow for this provider
    /// Returns None if the provider doesn't support authentication
    fn create_auth_flow(
        &self,
        _storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>> {
        None
    }
}
