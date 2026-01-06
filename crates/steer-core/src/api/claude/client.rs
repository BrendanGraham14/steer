use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum_macros::Display;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::api::error::StreamError;
use crate::api::provider::{CompletionStream, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::api::{CompletionResponse, Provider, error::ApiError};
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, ThoughtContent, ToolResult, UserContent,
};
use crate::auth::{
    AuthFlowWrapper, AuthStorage, DynAuthenticationFlow, InteractiveAuth,
    anthropic::{AnthropicOAuth, AnthropicOAuthFlow, refresh_if_needed},
};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::{ToolCall, ToolSchema};

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
pub enum AuthMethod {
    ApiKey(String),
    OAuth(Arc<dyn AuthStorage>),
}

#[derive(Clone)]
pub struct AnthropicClient {
    http_client: reqwest::Client,
    auth: AuthMethod,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum ThinkingType {
    Enabled,
}

impl Default for ThinkingType {
    fn default() -> Self {
        Self::Enabled
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Thinking {
    #[serde(rename = "type", default)]
    thinking_type: ThinkingType,
    budget_tokens: u32,
}

#[derive(Debug, Serialize, Clone)]
struct SystemContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(untagged)]
enum System {
    // Structured system prompt represented as a list of content blocks
    Content(Vec<SystemContentBlock>),
}

#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ClaudeMessage>,
    max_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<System>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Thinking>,
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
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        signature: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        #[serde(flatten)]
        extra: std::collections::HashMap<String, serde_json::Value>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        data: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: ClaudeMessageStart },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ClaudeContentBlockStart,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: ClaudeDelta },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: ClaudeMessageDeltaData,
        #[serde(default)]
        usage: Option<ClaudeUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ClaudeStreamError },
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageStart {
    #[serde(default)]
    id: String,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeContentBlockStart {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ClaudeDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Debug, Deserialize)]
struct ClaudeMessageDeltaData {
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeStreamError {
    #[serde(default)]
    message: String,
    #[serde(rename = "type", default)]
    error_type: String,
}

impl AnthropicClient {
    pub fn new(api_key: &str) -> Self {
        Self::with_api_key(api_key)
    }

    pub fn with_api_key(api_key: &str) -> Self {
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
            auth: AuthMethod::ApiKey(api_key.to_string()),
        }
    }

    pub fn with_oauth(storage: Arc<dyn AuthStorage>) -> Self {
        // For OAuth, we don't set default headers since they're dynamic
        let mut headers = header::HeaderMap::new();
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
            auth: AuthMethod::OAuth(storage),
        }
    }

    async fn get_auth_headers(&self) -> Result<Vec<(String, String)>, ApiError> {
        match &self.auth {
            AuthMethod::ApiKey(key) => Ok(vec![("x-api-key".to_string(), key.clone())]),
            AuthMethod::OAuth(storage) => {
                let oauth_client = AnthropicOAuth::new();
                let tokens = refresh_if_needed(storage, &oauth_client)
                    .await
                    .map_err(|e| ApiError::AuthError(e.to_string()))?;
                Ok(crate::auth::anthropic::get_oauth_headers(
                    &tokens.access_token,
                ))
            }
        }
    }
}

// Conversion functions start
fn convert_messages(messages: Vec<AppMessage>) -> Result<Vec<ClaudeMessage>, ApiError> {
    let claude_messages: Result<Vec<ClaudeMessage>, ApiError> =
        messages.into_iter().map(convert_single_message).collect();

    // Filter out any User messages that have empty content after removing app commands
    claude_messages.map(|messages| {
        messages
            .into_iter()
            .filter(|msg| {
                match &msg.content {
                    ClaudeMessageContent::Text { content } => !content.trim().is_empty(),
                    _ => true, // Keep all non-text messages
                }
            })
            .collect()
    })
}

fn convert_single_message(msg: AppMessage) -> Result<ClaudeMessage, ApiError> {
    match &msg.data {
        crate::app::conversation::MessageData::User { content, .. } => {
            // Convert UserContent to Claude format
            let combined_text = content
                .iter()
                .filter_map(|user_content| match user_content {
                    UserContent::Text { text } => Some(text.clone()),
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => Some(UserContent::format_command_execution_as_xml(
                        command, stdout, stderr, *exit_code,
                    )),
                    UserContent::AppCommand { .. } => {
                        // Don't send app commands to the model - they're for local execution only
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");

            Ok(ClaudeMessage {
                role: ClaudeMessageRole::User,
                content: ClaudeMessageContent::Text {
                    content: combined_text,
                },
                id: Some(msg.id.clone()),
            })
        }
        crate::app::conversation::MessageData::Assistant { content, .. } => {
            // Convert AssistantContent to Claude blocks
            let claude_blocks: Vec<ClaudeContentBlock> = content
                .iter()
                .filter_map(|assistant_content| match assistant_content {
                    AssistantContent::Text { text } => {
                        if text.trim().is_empty() {
                            None
                        } else {
                            Some(ClaudeContentBlock::Text {
                                text: text.clone(),
                                cache_control: None,
                                extra: Default::default(),
                            })
                        }
                    }
                    AssistantContent::ToolCall { tool_call } => Some(ClaudeContentBlock::ToolUse {
                        id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        input: tool_call.parameters.clone(),
                        cache_control: None,
                        extra: Default::default(),
                    }),
                    AssistantContent::Thought { thought } => {
                        match thought {
                            ThoughtContent::Signed { text, signature } => {
                                Some(ClaudeContentBlock::Thinking {
                                    thinking: text.clone(),
                                    signature: signature.clone(),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                            ThoughtContent::Redacted { data } => {
                                Some(ClaudeContentBlock::RedactedThinking {
                                    data: data.clone(),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                            ThoughtContent::Simple { text } => {
                                // Claude doesn't have a simple thought type, convert to text
                                Some(ClaudeContentBlock::Text {
                                    text: format!("[Thought: {text}]"),
                                    cache_control: None,
                                    extra: Default::default(),
                                })
                            }
                        }
                    }
                })
                .collect();

            if !claude_blocks.is_empty() {
                let claude_content = if claude_blocks.len() == 1 {
                    if let Some(ClaudeContentBlock::Text { text, .. }) = claude_blocks.first() {
                        ClaudeMessageContent::Text {
                            content: text.clone(),
                        }
                    } else {
                        ClaudeMessageContent::StructuredContent {
                            content: ClaudeStructuredContent(claude_blocks),
                        }
                    }
                } else {
                    ClaudeMessageContent::StructuredContent {
                        content: ClaudeStructuredContent(claude_blocks),
                    }
                };

                Ok(ClaudeMessage {
                    role: ClaudeMessageRole::Assistant,
                    content: claude_content,
                    id: Some(msg.id.clone()),
                })
            } else {
                debug!("No content blocks found: {:?}", content);
                Err(ApiError::InvalidRequest {
                    provider: "anthropic".to_string(),
                    details: format!(
                        "Assistant message ID {} resulted in no valid content blocks",
                        msg.id
                    ),
                })
            }
        }
        crate::app::conversation::MessageData::Tool {
            tool_use_id,
            result,
            ..
        } => {
            // Convert ToolResult to Claude format
            // Claude expects tool results as User messages
            let (result_text, is_error) = match result {
                ToolResult::Error(e) => (e.to_string(), Some(true)),
                _ => {
                    // For all other variants, use llm_format
                    let text = result.llm_format();
                    let text = if text.trim().is_empty() {
                        "(No output)".to_string()
                    } else {
                        text
                    };
                    (text, None)
                }
            };

            let claude_blocks = vec![ClaudeContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: vec![ClaudeContentBlock::Text {
                    text: result_text,
                    cache_control: None,
                    extra: Default::default(),
                }],
                is_error,
                cache_control: None,
                extra: Default::default(),
            }];

            Ok(ClaudeMessage {
                role: ClaudeMessageRole::User, // Tool results are sent as User messages in Claude
                content: ClaudeMessageContent::StructuredContent {
                    content: ClaudeStructuredContent(claude_blocks),
                },
                id: Some(msg.id.clone()),
            })
        }
    }
}
// Conversion functions end

// Convert Claude's content blocks to our provider-agnostic format
fn convert_claude_content(claude_blocks: Vec<ClaudeContentBlock>) -> Vec<AssistantContent> {
    claude_blocks
        .into_iter()
        .filter_map(|block| match block {
            ClaudeContentBlock::Text { text, .. } => Some(AssistantContent::Text { text }),
            ClaudeContentBlock::ToolUse {
                id, name, input, ..
            } => Some(AssistantContent::ToolCall {
                tool_call: steer_tools::ToolCall {
                    id,
                    name,
                    parameters: input,
                },
            }),
            ClaudeContentBlock::ToolResult { .. } => {
                warn!("Unexpected ToolResult block received in Claude response content");
                None
            }
            ClaudeContentBlock::Thinking {
                thinking,
                signature,
                ..
            } => Some(AssistantContent::Thought {
                thought: ThoughtContent::Signed {
                    text: thinking,
                    signature,
                },
            }),
            ClaudeContentBlock::RedactedThinking { data, .. } => Some(AssistantContent::Thought {
                thought: ThoughtContent::Redacted { data },
            }),
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
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
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

        let system_content = match (system, &self.auth) {
            (Some(sys), AuthMethod::ApiKey(_)) => Some(System::Content(vec![SystemContentBlock {
                content_type: "text".to_string(),
                text: sys,
                cache_control: cache_setting.clone(),
            }])),
            (Some(sys), AuthMethod::OAuth(_)) => Some(System::Content(vec![
                SystemContentBlock {
                    content_type: "text".to_string(),
                    text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
                    cache_control: cache_setting.clone(),
                },
                SystemContentBlock {
                    content_type: "text".to_string(),
                    text: sys,
                    cache_control: cache_setting.clone(),
                },
            ])),
            (None, AuthMethod::ApiKey(_)) => None,
            (None, AuthMethod::OAuth(_)) => Some(System::Content(vec![SystemContentBlock {
                content_type: "text".to_string(),
                text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
                cache_control: cache_setting.clone(),
            }])),
        };

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

        // Extract model-specific logic using ModelId
        let supports_thinking = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .map(|tc| tc.enabled)
            .unwrap_or(false);

        let request = if supports_thinking {
            // Use catalog/call options to configure thinking budget when provided
            let budget = call_options
                .as_ref()
                .and_then(|o| o.thinking_config)
                .and_then(|tc| tc.budget_tokens)
                .unwrap_or(4000);
            let thinking = Some(Thinking {
                thinking_type: ThinkingType::Enabled,
                budget_tokens: budget,
            });
            CompletionRequest {
                model: model_id.1.clone(), // Use the model ID string
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map(|v| v as usize)
                    .unwrap_or(32_000),
                system: system_content.clone(),
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(1.0)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: None,
                thinking,
            }
        } else {
            CompletionRequest {
                model: model_id.1.clone(), // Use the model ID string
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map(|v| v as usize)
                    .unwrap_or(8000),
                system: system_content,
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(0.7)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: None,
                thinking: None,
            }
        };

        let auth_headers = self.get_auth_headers().await?;
        let mut request_builder = self.http_client.post(API_URL).json(&request);

        // Add dynamic auth headers
        for (name, value) in auth_headers {
            request_builder = request_builder.header(&name, &value);
        }

        // Check for thinking beta header based on model ID
        if supports_thinking && matches!(&self.auth, AuthMethod::ApiKey(_)) {
            request_builder =
                request_builder.header("anthropic-beta", "interleaved-thinking-2025-05-14");
        }

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "claude::complete", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string()});
            }
            res = request_builder.send() => {
                res?
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
                    text_res?
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
                return Err(ApiError::Cancelled { provider: self.name().to_string() });
            }
            text_res = response.text() => {
                text_res?
            }
        };

        let claude_completion: ClaudeCompletionResponse = serde_json::from_str(&response_text)
            .map_err(|e| ApiError::ResponseParsingError {
                provider: self.name().to_string(),
                details: format!("Error: {e}, Body: {response_text}"),
            })?;
        let completion = CompletionResponse {
            content: convert_claude_content(claude_completion.content),
        };

        Ok(completion)
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
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

        let system_content = match (system, &self.auth) {
            (Some(sys), AuthMethod::ApiKey(_)) => Some(System::Content(vec![SystemContentBlock {
                content_type: "text".to_string(),
                text: sys,
                cache_control: cache_setting.clone(),
            }])),
            (Some(sys), AuthMethod::OAuth(_)) => Some(System::Content(vec![
                SystemContentBlock {
                    content_type: "text".to_string(),
                    text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
                    cache_control: cache_setting.clone(),
                },
                SystemContentBlock {
                    content_type: "text".to_string(),
                    text: sys,
                    cache_control: cache_setting.clone(),
                },
            ])),
            (None, AuthMethod::ApiKey(_)) => None,
            (None, AuthMethod::OAuth(_)) => Some(System::Content(vec![SystemContentBlock {
                content_type: "text".to_string(),
                text: "You are Claude Code, Anthropic's official CLI for Claude.".to_string(),
                cache_control: cache_setting.clone(),
            }])),
        };

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

        let supports_thinking = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .map(|tc| tc.enabled)
            .unwrap_or(false);

        let request = if supports_thinking {
            let budget = call_options
                .as_ref()
                .and_then(|o| o.thinking_config)
                .and_then(|tc| tc.budget_tokens)
                .unwrap_or(4000);
            let thinking = Some(Thinking {
                thinking_type: ThinkingType::Enabled,
                budget_tokens: budget,
            });
            CompletionRequest {
                model: model_id.1.clone(),
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map(|v| v as usize)
                    .unwrap_or(32_000),
                system: system_content.clone(),
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(1.0)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: Some(true),
                thinking,
            }
        } else {
            CompletionRequest {
                model: model_id.1.clone(),
                messages: claude_messages,
                max_tokens: call_options
                    .as_ref()
                    .and_then(|o| o.max_tokens)
                    .map(|v| v as usize)
                    .unwrap_or(8000),
                system: system_content,
                tools,
                temperature: call_options
                    .as_ref()
                    .and_then(|o| o.temperature)
                    .or(Some(0.7)),
                top_p: call_options.as_ref().and_then(|o| o.top_p),
                top_k: None,
                stream: Some(true),
                thinking: None,
            }
        };

        let auth_headers = self.get_auth_headers().await?;
        let mut request_builder = self.http_client.post(API_URL).json(&request);

        for (name, value) in auth_headers {
            request_builder = request_builder.header(&name, &value);
        }

        if supports_thinking && matches!(&self.auth, AuthMethod::ApiKey(_)) {
            request_builder =
                request_builder.header("anthropic-beta", "interleaved-thinking-2025-05-14");
        }

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(ApiError::Cancelled { provider: self.name().to_string() });
            }
            res = request_builder.send() => {
                res?
            }
        };

        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await?;
            return Err(match status.as_u16() {
                401 | 403 => ApiError::AuthenticationFailed {
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

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        let stream = convert_claude_stream(sse_stream, token);

        Ok(Box::pin(stream))
    }
}

fn convert_claude_stream(
    sse_stream: crate::api::sse::SseStream,
    token: CancellationToken,
) -> impl futures_core::Stream<Item = StreamChunk> + Send {
    async_stream::stream! {
        let mut accumulated_content: Vec<AssistantContent> = vec![];
        let mut current_tool_id: Option<String> = None;
        let mut current_tool_name: Option<String> = None;
        let mut current_tool_input = String::new();
        let mut accumulated_text = String::new();
        let mut current_thinking = String::new();
        let mut current_thinking_signature: Option<String> = None;

        tokio::pin!(sse_stream);

        while let Some(event_result) = sse_stream.next().await {
            if token.is_cancelled() {
                yield StreamChunk::Error(StreamError::Cancelled);
                break;
            }

            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    yield StreamChunk::Error(StreamError::SseParse(e.to_string()));
                    break;
                }
            };

            let parsed: Result<ClaudeStreamEvent, _> = serde_json::from_str(&event.data);
            let stream_event = match parsed {
                Ok(e) => e,
                Err(_) => continue,
            };

            match stream_event {
                ClaudeStreamEvent::ContentBlockDelta {
                    delta: ClaudeDelta::TextDelta { text },
                    ..
                } => {
                    accumulated_text.push_str(&text);
                    yield StreamChunk::TextDelta(text);
                }
                ClaudeStreamEvent::ContentBlockDelta {
                    delta: ClaudeDelta::ThinkingDelta { thinking },
                    ..
                } => {
                    current_thinking.push_str(&thinking);
                    yield StreamChunk::ThinkingDelta(thinking);
                }
                ClaudeStreamEvent::ContentBlockDelta {
                    delta: ClaudeDelta::SignatureDelta { signature },
                    ..
                } => {
                    current_thinking_signature = Some(signature);
                }
                ClaudeStreamEvent::ContentBlockStart { content_block, .. } => {
                    if content_block.block_type == "tool_use" {
                        if let (Some(id), Some(name)) =
                            (content_block.id.clone(), content_block.name.clone())
                        {
                            current_tool_id = Some(id.clone());
                            current_tool_name = Some(name.clone());
                            yield StreamChunk::ToolUseStart { id, name };
                        }
                    } else if content_block.block_type == "thinking" {
                        current_thinking.clear();
                        current_thinking_signature = None;
                    }
                }
                ClaudeStreamEvent::ContentBlockDelta {
                    delta: ClaudeDelta::InputJsonDelta { partial_json },
                    ..
                } => {
                    if let Some(ref id) = current_tool_id {
                        current_tool_input.push_str(&partial_json);
                        yield StreamChunk::ToolUseInputDelta {
                            id: id.clone(),
                            delta: partial_json,
                        };
                    }
                }
                ClaudeStreamEvent::ContentBlockStop { index } => {
                    if let (Some(id), Some(name)) =
                        (current_tool_id.take(), current_tool_name.take())
                    {
                        let input: serde_json::Value =
                            serde_json::from_str(&current_tool_input)
                                .unwrap_or(serde_json::Value::Object(Default::default()));
                        accumulated_content.push(AssistantContent::ToolCall {
                            tool_call: ToolCall {
                                id,
                                name,
                                parameters: input,
                            },
                        });
                        current_tool_input.clear();
                    }
                    if !current_thinking.is_empty() {
                        let thought = if let Some(sig) = current_thinking_signature.take() {
                            ThoughtContent::Signed {
                                text: std::mem::take(&mut current_thinking),
                                signature: sig,
                            }
                        } else {
                            ThoughtContent::Simple {
                                text: std::mem::take(&mut current_thinking),
                            }
                        };
                        accumulated_content.push(AssistantContent::Thought { thought });
                    }
                    yield StreamChunk::ContentBlockStop { index };
                }
                ClaudeStreamEvent::MessageStop => {
                    if !accumulated_text.is_empty() {
                        accumulated_content.insert(
                            0,
                            AssistantContent::Text {
                                text: std::mem::take(&mut accumulated_text),
                            },
                        );
                    }
                    yield StreamChunk::MessageComplete(CompletionResponse {
                        content: accumulated_content.clone(),
                    });
                }
                ClaudeStreamEvent::Error { error } => {
                    yield StreamChunk::Error(StreamError::Provider {
                        provider: "anthropic".into(),
                        error_type: error.error_type,
                        message: error.message,
                    });
                }
                ClaudeStreamEvent::MessageStart { .. }
                | ClaudeStreamEvent::MessageDelta { .. }
                | ClaudeStreamEvent::Ping => {}
            }
        }
    }
}

impl InteractiveAuth for AnthropicClient {
    fn create_auth_flow(
        &self,
        storage: Arc<dyn AuthStorage>,
    ) -> Option<Box<dyn DynAuthenticationFlow>> {
        Some(Box::new(AuthFlowWrapper::new(AnthropicOAuthFlow::new(
            storage,
        ))))
    }
}
