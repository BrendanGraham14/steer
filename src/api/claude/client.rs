use anyhow::Result;
use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use strum_macros::Display;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::messages::{
    ContentBlock, Message as ApiMessage, MessageContent as ApiMessageContent,
    MessageRole as ApiMessageRole, StructuredContent as ApiStructuredContent,
};
use crate::api::provider::{CompletionResponse, Provider};
use crate::api::tools::Tool;
const API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Display)]
pub enum ClaudeMessageRole {
    #[serde(rename = "user")]
    #[strum(serialize = "user")]
    User,
    #[serde(rename = "assistant")]
    #[strum(serialize = "assistant")]
    Assistant,
    #[serde(rename = "tool")]
    #[strum(serialize = "tool")]
    Tool,
}

/// Represents a message to be sent to the Claude API
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeMessage {
    pub role: ClaudeMessageRole,
    #[serde(flatten)]
    pub content: ClaudeMessageContent,
    #[serde(skip_serializing)]
    pub id: Option<String>,
}

/// Content types for Claude API messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ClaudeMessageContent {
    /// Simple text content
    Text { content: String },
    /// Structured content for tool results or other special content
    StructuredContent { content: ClaudeStructuredContent },
}

/// Represents structured content blocks for messages
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ClaudeStructuredContent(pub Vec<ClaudeContentBlock>);

#[derive(Clone)]
pub struct AnthropicClient {
    http_client: reqwest::Client,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ClaudeMessage>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ClaudeCompletionResponse {
    id: String,
    content: Vec<ClaudeContentBlock>,
    model: String,
    role: String,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
    #[serde(default)]
    usage: ClaudeUsage,
    // Allow other fields for API flexibility
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

fn default_cache_type() -> String {
    "ephemeral".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CacheControl {
    #[serde(rename = "type", default = "default_cache_type")]
    cache_type: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Vec<ClaudeContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
struct ClaudeUsage {
    input_tokens: usize,
    output_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_creation_input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<usize>,
}

impl AnthropicClient {
    pub fn new(api_key: &str) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert("x-api-key", header::HeaderValue::from_str(api_key).unwrap());
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_static("2023-06-01"),
        );
        headers.insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/json"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http_client: client,
        }
    }
}

// Conversion functions start
fn convert_messages(messages: Vec<ApiMessage>) -> Result<Vec<ClaudeMessage>, ApiError> {
    messages.into_iter().map(convert_single_message).collect()
}

fn convert_single_message(msg: ApiMessage) -> Result<ClaudeMessage, ApiError> {
    let claude_role = convert_role(msg.role).map_err(|e| ApiError::InvalidRequest {
        provider: "anthropic".to_string(),
        details: format!(
            "Failed to convert role for message with id {:?}: {}",
            msg.id, e
        ),
    })?;
    let claude_content = convert_content(msg.content).map_err(|e| ApiError::InvalidRequest {
        provider: "anthropic".to_string(),
        details: format!(
            "Failed to convert content for message with id {:?}: {}",
            msg.id, e
        ),
    })?;

    Ok(ClaudeMessage {
        role: claude_role,
        content: claude_content,
        id: msg.id,
    })
}

fn convert_role(role: ApiMessageRole) -> Result<ClaudeMessageRole, ApiError> {
    match role {
        ApiMessageRole::User => Ok(ClaudeMessageRole::User),
        ApiMessageRole::Assistant => Ok(ClaudeMessageRole::Assistant),
        ApiMessageRole::Tool => Ok(ClaudeMessageRole::User),
    }
}

fn convert_content(content: ApiMessageContent) -> Result<ClaudeMessageContent, ApiError> {
    match content {
        ApiMessageContent::Text { content } => Ok(ClaudeMessageContent::Text { content }),
        ApiMessageContent::StructuredContent {
            content: structured,
        } => convert_structured_content(structured).map(|claude_structured| {
            ClaudeMessageContent::StructuredContent {
                content: claude_structured,
            }
        }),
    }
}

fn convert_structured_content(
    structured: ApiStructuredContent,
) -> Result<ClaudeStructuredContent, ApiError> {
    let converted_blocks: Result<Vec<ClaudeContentBlock>, ApiError> =
        structured.0.into_iter().map(convert_block).collect();

    converted_blocks.map(ClaudeStructuredContent)
}

fn convert_block(block: ContentBlock) -> Result<ClaudeContentBlock, ApiError> {
    match block {
        ContentBlock::Text { text } => Ok(ClaudeContentBlock::Text {
            text,
            cache_control: None,
            extra: Default::default(),
        }),
        ContentBlock::ToolUse { id, name, input } => Ok(ClaudeContentBlock::ToolUse {
            id,
            name,
            input,
            cache_control: None,
            extra: Default::default(),
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let claude_inner_content: Result<Vec<ClaudeContentBlock>, ApiError> =
                content.into_iter().map(convert_block).collect();

            claude_inner_content.map(|inner_blocks| ClaudeContentBlock::ToolResult {
                tool_use_id,
                content: inner_blocks,
                is_error,
                cache_control: None,
                extra: Default::default(),
            })
        }
    }
}
// Conversion functions end

// Convert Claude's content blocks to our provider-agnostic format
fn convert_claude_content(claude_blocks: Vec<ClaudeContentBlock>) -> Vec<ContentBlock> {
    claude_blocks
        .into_iter()
        .filter_map(|block| match block {
            ClaudeContentBlock::Text { text, .. } => Some(ContentBlock::Text { text }),
            ClaudeContentBlock::ToolUse {
                id, name, input, ..
            } => Some(ContentBlock::ToolUse { id, name, input }),
            ClaudeContentBlock::ToolResult { .. } => {
                warn!("Unexpected ToolResult block received in Claude response content");
                None
            }
            ClaudeContentBlock::Unknown => {
                warn!("Unknown content block received in Claude response content");
                None
            }
        })
        .collect()
}

#[async_trait]
impl Provider for AnthropicClient {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<ApiMessage>,
        system: Option<String>,
        tools: Option<Vec<Tool>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let mut claude_messages = convert_messages(messages)?;

        if claude_messages.is_empty() {
            return Err(ApiError::InvalidRequest {
                provider: self.name().to_string(),
                details: "No messages provided".to_string(),
            });
        }

        let last_message = claude_messages.last_mut().unwrap();
        let cache_setting = Some(CacheControl {
            cache_type: "ephemeral".to_string(),
        });

        match &mut last_message.content {
            ClaudeMessageContent::StructuredContent { content } => {
                for block in content.0.iter_mut() {
                    if let ClaudeContentBlock::ToolResult { cache_control, .. } = block {
                        *cache_control = cache_setting.clone();
                    }
                }
            }
            ClaudeMessageContent::Text { content } => {
                let text_content = content.clone();
                last_message.content = ClaudeMessageContent::StructuredContent {
                    content: ClaudeStructuredContent(vec![ClaudeContentBlock::Text {
                        text: text_content,
                        cache_control: cache_setting,
                        extra: Default::default(),
                    }]),
                };
            }
        }

        let request = CompletionRequest {
            model: model.as_ref().to_string(),
            messages: claude_messages,
            max_tokens: 4000,
            system,
            tools,
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            stream: None,
        };

        debug!(target: "API Request", "{:?}", request);

        match serde_json::to_string_pretty(&request) {
            Ok(json_payload) => {
                debug!(target: "Full API Request Payload (JSON)", "{}", json_payload);
            }
            Err(e) => {
                error!(target: "API Request Serialization Error", "Failed to serialize request to JSON: {}", e);
            }
        }

        let request_builder = self.http_client.post(API_URL).json(&request);

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "claude::complete", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string()});
            }
            res = request_builder.send() => {
                res.map_err(ApiError::Network)?
            }
        };

        if token.is_cancelled() {
            debug!(target: "claude::complete", "Cancellation token triggered after sending request, before status check.");
            return Err(ApiError::Cancelled {
                provider: self.name().to_string(),
            });
        }

        let status = response.status();
        if !status.is_success() {
            let error_text = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    debug!(target: "claude::complete", "Cancellation token triggered while reading error response body.");
                    return Err(ApiError::Cancelled{ provider: self.name().to_string()});
                }
                text_res = response.text() => {
                    text_res.map_err(ApiError::Network)?
                }
            };
            return Err(match status.as_u16() {
                401 => ApiError::AuthenticationFailed {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                403 => ApiError::AuthenticationFailed {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                429 => ApiError::RateLimited {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                400..=499 => ApiError::InvalidRequest {
                    provider: self.name().to_string(),
                    details: error_text,
                },
                500..=599 => ApiError::ServerError {
                    provider: self.name().to_string(),
                    status_code: status.as_u16(),
                    details: error_text,
                },
                _ => ApiError::Unknown {
                    provider: self.name().to_string(),
                    details: error_text,
                },
            });
        }

        let response_text = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "claude::complete", "Cancellation token triggered while reading successful response body.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string()});
            }
            text_res = response.text() => {
                text_res.map_err(ApiError::Network)?
            }
        };

        let claude_completion: ClaudeCompletionResponse = serde_json::from_str(&response_text)
            .map_err(|e| ApiError::ResponseParsingError {
                provider: self.name().to_string(),
                details: format!("Error: {}, Body: {}", e, response_text),
            })?;
        let completion = CompletionResponse {
            content: convert_claude_content(claude_completion.content),
        };

        Ok(completion)
    }
}
