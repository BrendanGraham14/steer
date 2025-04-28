use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Debug;
use tokio_util::sync::CancellationToken;

use crate::api::messages::Message;
use crate::api::tools::Tool;

use super::Model;

/// Represents a content block in a message from any provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    /// Text content
    Text {
        text: String,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, Value>,
    },
    /// Tool use content
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, Value>,
    },
    /// Unknown content type (for forward compatibility)
    Unknown,
}

/// Response from the provider's completion API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: Vec<ContentBlock>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl CompletionResponse {
    /// Extract all text content from the response
    pub fn extract_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
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
            .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
    }

    /// Extract all tool calls from the response
    pub fn extract_tool_calls(&self) -> Vec<crate::api::tools::ToolCall> {
        self.content
            .iter()
            .filter_map(|block| {
                if let ContentBlock::ToolUse {
                    id, name, input, ..
                } = block
                {
                    Some(crate::api::tools::ToolCall {
                        name: name.clone(),
                        parameters: input.clone(),
                        id: id.clone(),
                    })
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
        tools: Option<Vec<Tool>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse>;
}
