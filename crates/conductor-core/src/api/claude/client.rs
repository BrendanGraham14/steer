use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use strum_macros::Display;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::api::{CompletionResponse, Model, Provider, error::ApiError};
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, ThoughtContent, ToolResult, UserContent,
};
use crate::auth::{
    AuthFlowWrapper, AuthStorage, DynAuthenticationFlow, InteractiveAuth,
    anthropic::{AnthropicOAuth, AnthropicOAuthFlow, refresh_if_needed},
};
use conductor_tools::ToolSchema;
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
    match msg {
        AppMessage::User { content, id, .. } => {
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
                id: Some(id),
            })
        }
        AppMessage::Assistant { content, id, .. } => {
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
                    id: Some(id),
                })
            } else {
                debug!("No content blocks found: {:?}", content);
                Err(ApiError::InvalidRequest {
                    provider: "anthropic".to_string(),
                    details: format!(
                        "Assistant message ID {id} resulted in no valid content blocks"
                    ),
                })
            }
        }
        AppMessage::Tool {
            tool_use_id,
            result,
            id,
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
                tool_use_id,
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
                id: Some(id),
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
                tool_call: conductor_tools::ToolCall {
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
        model: Model,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
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

        let request = if model.supports_thinking() {
            let thinking = Some(Thinking {
                thinking_type: ThinkingType::Enabled,
                budget_tokens: 4000,
            });
            CompletionRequest {
                model: model.as_ref().to_string(),
                messages: claude_messages,
                max_tokens: 32_000,
                system: system_content.clone(),
                tools,
                temperature: Some(1.0),
                top_p: None,
                top_k: None,
                stream: None,
                thinking,
            }
        } else {
            CompletionRequest {
                model: model.as_ref().to_string(),
                messages: claude_messages,
                max_tokens: 8000,
                system: system_content,
                tools,
                temperature: Some(0.7),
                top_p: None,
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

        if let (
            Model::ClaudeSonnet4_20250514 | Model::ClaudeOpus4_20250514,
            AuthMethod::ApiKey(_),
        ) = (&model, &self.auth)
        {
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
