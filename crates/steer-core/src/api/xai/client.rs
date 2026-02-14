use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::error::{ApiError, SseParseError, StreamError};
use crate::api::provider::{
    CompletionResponse, CompletionStream, Provider, StreamChunk, TokenUsage,
};
use crate::api::sse::parse_sse_stream;
use crate::api::util::{map_http_status_to_api_error, normalize_chat_url};
use crate::app::SystemContext;
use crate::app::conversation::{
    AssistantContent, ImageSource, Message as AppMessage, ToolResult, UserContent,
};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::ToolSchema;

const DEFAULT_API_URL: &str = "https://api.x.ai/v1/chat/completions";

#[derive(Clone)]
pub struct XAIClient {
    http_client: reqwest::Client,
    base_url: String,
}

// xAI-specific message format (similar to OpenAI but with some differences)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum XAIMessage {
    System {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    User {
        content: XAIUserContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<XAIToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Tool {
        content: String,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum XAIUserContent {
    Text(String),
    Parts(Vec<XAIUserContentPart>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum XAIUserContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: XAIImageUrl },
}

#[derive(Debug, Serialize, Deserialize)]
struct XAIImageUrl {
    url: String,
}

// xAI function calling format
#[derive(Debug, Serialize, Deserialize)]
struct XAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// xAI tool format
#[derive(Debug, Serialize, Deserialize)]
struct XAITool {
    #[serde(rename = "type")]
    tool_type: String, // "function"
    function: XAIFunction,
}

// xAI tool call
#[derive(Debug, Serialize, Deserialize)]
struct XAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: XAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct XAIFunctionCall {
    name: String,
    arguments: String, // JSON string
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReasoningEffort {
    Low,
    High,
}

#[derive(Debug, Serialize, Deserialize)]
struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    include_usage: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ToolChoice {
    String(String), // "auto", "required", "none"
    Specific {
        #[serde(rename = "type")]
        tool_type: String,
        function: ToolChoiceFunction,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ToolChoiceFunction {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    from_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_search_results: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_citations: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sources: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WebSearchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    search_context_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_location: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<XAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deferred: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logit_bias: Option<HashMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logprobs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_parameters: Option<SearchParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<XAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_logprobs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_search_options: Option<WebSearchOptions>,
}

#[derive(Debug, Serialize, Deserialize)]
struct XAICompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<XAIUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    citations: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debug_output: Option<DebugOutput>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Choice {
    index: i32,
    message: AssistantMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AssistantMessage {
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<XAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptTokensDetails {
    #[serde(rename = "cached_tokens")]
    cached: usize,
    #[serde(rename = "audio_tokens")]
    audio: usize,
    #[serde(rename = "image_tokens")]
    image: usize,
    #[serde(rename = "text_tokens")]
    text: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionTokensDetails {
    #[serde(rename = "reasoning_tokens")]
    reasoning: usize,
    #[serde(rename = "audio_tokens")]
    audio: usize,
    #[serde(rename = "accepted_prediction_tokens")]
    accepted_prediction: usize,
    #[serde(rename = "rejected_prediction_tokens")]
    rejected_prediction: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct XAIUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_sources_used: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DebugOutput {
    attempts: usize,
    cache_read_count: usize,
    cache_read_input_bytes: usize,
    cache_write_count: usize,
    cache_write_input_bytes: usize,
    prompt: String,
    request: String,
    responses: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct XAIStreamChunk {
    #[expect(dead_code)]
    id: String,
    choices: Vec<XAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<XAIUsage>,
}

#[derive(Debug, Deserialize)]
struct XAIStreamChoice {
    #[expect(dead_code)]
    index: u32,
    delta: XAIStreamDelta,
    #[expect(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XAIStreamDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<XAIStreamToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct XAIStreamToolCall {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<XAIStreamFunction>,
}

#[derive(Debug, Deserialize)]
struct XAIStreamFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

impl XAIClient {
    pub fn new(api_key: String) -> Result<Self, ApiError> {
        Self::with_base_url(api_key, None)
    }

    pub fn with_base_url(api_key: String, base_url: Option<String>) -> Result<Self, ApiError> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|e| {
                ApiError::AuthenticationFailed {
                    provider: "xai".to_string(),
                    details: format!("Invalid API key: {e}"),
                }
            })?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout
            .build()
            .map_err(ApiError::Network)?;

        let base_url = normalize_chat_url(base_url.as_deref(), DEFAULT_API_URL);

        Ok(Self {
            http_client: client,
            base_url,
        })
    }

    fn user_image_part(
        image: &crate::app::conversation::ImageContent,
    ) -> Result<XAIUserContentPart, ApiError> {
        let image_url = match &image.source {
            ImageSource::DataUrl { data_url } => data_url.clone(),
            ImageSource::Url { url } => url.clone(),
            ImageSource::SessionFile { relative_path } => {
                return Err(ApiError::UnsupportedFeature {
                    provider: "xai".to_string(),
                    feature: "image input source".to_string(),
                    details: format!(
                        "xAI chat API cannot access session file '{}' directly; use data URLs or public URLs",
                        relative_path
                    ),
                });
            }
        };

        Ok(XAIUserContentPart::ImageUrl {
            image_url: XAIImageUrl { url: image_url },
        })
    }

    fn convert_messages(
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
    ) -> Result<Vec<XAIMessage>, ApiError> {
        let mut xai_messages = Vec::new();

        // Add system message if provided
        if let Some(system_content) = system.and_then(|context| context.render()) {
            xai_messages.push(XAIMessage::System {
                content: system_content,
                name: None,
            });
        }

        // Convert our messages to xAI format
        for message in messages {
            match &message.data {
                crate::app::conversation::MessageData::User { content, .. } => {
                    let mut content_parts = Vec::new();

                    for user_content in content {
                        match user_content {
                            UserContent::Text { text } => {
                                content_parts.push(XAIUserContentPart::Text { text: text.clone() });
                            }
                            UserContent::Image { image } => {
                                content_parts.push(Self::user_image_part(image)?);
                            }
                            UserContent::CommandExecution {
                                command,
                                stdout,
                                stderr,
                                exit_code,
                            } => {
                                content_parts.push(XAIUserContentPart::Text {
                                    text: UserContent::format_command_execution_as_xml(
                                        command, stdout, stderr, *exit_code,
                                    ),
                                });
                            }
                        }
                    }

                    // Only add the message if it has content after filtering
                    if !content_parts.is_empty() {
                        let content = match content_parts.as_slice() {
                            [XAIUserContentPart::Text { text }] => {
                                XAIUserContent::Text(text.clone())
                            }
                            _ => XAIUserContent::Parts(content_parts),
                        };

                        xai_messages.push(XAIMessage::User {
                            content,
                            name: None,
                        });
                    }
                }
                crate::app::conversation::MessageData::Assistant { content, .. } => {
                    // Convert AssistantContent to xAI format
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();

                    for content_block in content {
                        match content_block {
                            AssistantContent::Text { text } => {
                                text_parts.push(text.clone());
                            }
                            AssistantContent::Image { image } => {
                                match Self::user_image_part(image) {
                                    Ok(XAIUserContentPart::ImageUrl { image_url }) => {
                                        text_parts.push(format!("[Image URL: {}]", image_url.url));
                                    }
                                    Ok(XAIUserContentPart::Text { text }) => {
                                        text_parts.push(text);
                                    }
                                    Err(err) => {
                                        debug!(
                                            target: "xai::convert_messages",
                                            "Skipping unsupported assistant image block: {}",
                                            err
                                        );
                                    }
                                }
                            }
                            AssistantContent::ToolCall { tool_call, .. } => {
                                tool_calls.push(XAIToolCall {
                                    id: tool_call.id.clone(),
                                    tool_type: "function".to_string(),
                                    function: XAIFunctionCall {
                                        name: tool_call.name.clone(),
                                        arguments: tool_call.parameters.to_string(),
                                    },
                                });
                            }
                            AssistantContent::Thought { .. } => {

                                // xAI doesn't support thinking blocks in requests, only in responses
                            }
                        }
                    }

                    // Build the assistant message
                    let content = if text_parts.is_empty() {
                        None
                    } else {
                        Some(text_parts.join("\n"))
                    };

                    let tool_calls_opt = if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    };

                    xai_messages.push(XAIMessage::Assistant {
                        content,
                        tool_calls: tool_calls_opt,
                        name: None,
                    });
                }
                crate::app::conversation::MessageData::Tool {
                    tool_use_id,
                    result,
                    ..
                } => {
                    // Convert ToolResult to xAI format
                    let content_text = if let ToolResult::Error(e) = result {
                        format!("Error: {e}")
                    } else {
                        let text = result.llm_format();
                        if text.trim().is_empty() {
                            "(No output)".to_string()
                        } else {
                            text
                        }
                    };

                    xai_messages.push(XAIMessage::Tool {
                        content: content_text,
                        tool_call_id: tool_use_id.clone(),
                        name: None,
                    });
                }
            }
        }

        Ok(xai_messages)
    }

    fn convert_tools(tools: Vec<ToolSchema>) -> Vec<XAITool> {
        tools
            .into_iter()
            .map(|tool| XAITool {
                tool_type: "function".to_string(),
                function: XAIFunction {
                    name: tool.name,
                    description: tool.description,
                    parameters: tool.input_schema.as_value().clone(),
                },
            })
            .collect()
    }
}

fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn map_xai_usage(usage: &XAIUsage) -> TokenUsage {
    TokenUsage::new(
        saturating_u32(usage.prompt_tokens),
        saturating_u32(usage.completion_tokens),
        saturating_u32(usage.total_tokens),
    )
}

fn convert_xai_completion_response(
    xai_response: XAICompletionResponse,
) -> Result<CompletionResponse, ApiError> {
    let usage = xai_response.usage.as_ref().map(map_xai_usage);

    let Some(choice) = xai_response.choices.first() else {
        return Err(ApiError::NoChoices {
            provider: "xai".to_string(),
        });
    };

    let mut content_blocks = Vec::new();

    // Add reasoning content (thinking) first if present
    if let Some(reasoning) = &choice.message.reasoning_content
        && !reasoning.trim().is_empty()
    {
        content_blocks.push(AssistantContent::Thought {
            thought: crate::app::conversation::ThoughtContent::Simple {
                text: reasoning.clone(),
            },
        });
    }

    // Add regular content
    if let Some(content) = &choice.message.content
        && !content.trim().is_empty()
    {
        content_blocks.push(AssistantContent::Text {
            text: content.clone(),
        });
    }

    // Add tool calls
    if let Some(tool_calls) = &choice.message.tool_calls {
        for tool_call in tool_calls {
            // Parse the arguments JSON string
            let parameters = serde_json::from_str(&tool_call.function.arguments)
                .unwrap_or(serde_json::Value::Null);

            content_blocks.push(AssistantContent::ToolCall {
                tool_call: steer_tools::ToolCall {
                    id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    parameters,
                },
                thought_signature: None,
            });
        }
    }

    Ok(CompletionResponse {
        content: content_blocks,
        usage,
    })
}

#[async_trait]
impl Provider for XAIClient {
    fn name(&self) -> &'static str {
        "xai"
    }

    async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let xai_messages = Self::convert_messages(messages, system)?;
        let xai_tools = tools.map(Self::convert_tools);

        // Extract thinking support and map optional effort
        let (supports_thinking, reasoning_effort) = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config)
            .map_or((false, None), |tc| {
                let effort = tc.effort.map(|e| match e {
                    crate::config::toml_types::ThinkingEffort::Low => ReasoningEffort::Low,
                    crate::config::toml_types::ThinkingEffort::Medium => ReasoningEffort::High, // xAI has Low/High only
                    crate::config::toml_types::ThinkingEffort::High => ReasoningEffort::High,
                    crate::config::toml_types::ThinkingEffort::XHigh => ReasoningEffort::High, // xAI has Low/High only
                });
                (tc.enabled, effort)
            });

        // grok-4 supports thinking by default but not the reasoning_effort parameter
        let reasoning_effort = if supports_thinking && model_id.id != "grok-4-0709" {
            reasoning_effort.or(Some(ReasoningEffort::High))
        } else {
            None
        };

        let request = CompletionRequest {
            model: model_id.id.clone(), // Use the model ID string
            messages: xai_messages,
            deferred: None,
            frequency_penalty: None,
            logit_bias: None,
            logprobs: None,
            max_completion_tokens: Some(32768),
            max_tokens: None,
            n: None,
            parallel_tool_calls: None,
            presence_penalty: None,
            reasoning_effort,
            response_format: None,
            search_parameters: None,
            seed: None,
            stop: None,
            stream: None,
            stream_options: None,
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            tool_choice: None,
            tools: xai_tools,
            top_logprobs: None,
            top_p: call_options.as_ref().and_then(|o| o.top_p),
            user: None,
            web_search_options: None,
        };

        let response = self
            .http_client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .map_err(ApiError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| String::new());

            debug!(
                target: "grok::complete",
                "Grok API error - Status: {}, Body: {}",
                status,
                error_text
            );

            return Err(map_http_status_to_api_error(
                self.name(),
                status.as_u16(),
                error_text,
            ));
        }

        let response_text = tokio::select! {
            () = token.cancelled() => {
                debug!(target: "grok::complete", "Cancellation token triggered while reading successful response body.");
                return Err(ApiError::Cancelled { provider: self.name().to_string() });
            }
            text_res = response.text() => {
                text_res?
            }
        };

        let xai_response: XAICompletionResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                error!(
                    target: "xai::complete",
                    "Failed to parse response: {}, Body: {}",
                    e,
                    response_text
                );
                ApiError::ResponseParsingError {
                    provider: self.name().to_string(),
                    details: format!("Error: {e}, Body: {response_text}"),
                }
            })?;

        convert_xai_completion_response(xai_response).map_err(|err| match err {
            ApiError::NoChoices { .. } => ApiError::NoChoices {
                provider: self.name().to_string(),
            },
            other => other,
        })
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let xai_messages = Self::convert_messages(messages, system)?;
        let xai_tools = tools.map(Self::convert_tools);

        let (supports_thinking, reasoning_effort) = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config)
            .map_or((false, None), |tc| {
                let effort = tc.effort.map(|e| match e {
                    crate::config::toml_types::ThinkingEffort::Low => ReasoningEffort::Low,
                    crate::config::toml_types::ThinkingEffort::Medium => ReasoningEffort::High,
                    crate::config::toml_types::ThinkingEffort::High => ReasoningEffort::High,
                    crate::config::toml_types::ThinkingEffort::XHigh => ReasoningEffort::High, // xAI has Low/High only
                });
                (tc.enabled, effort)
            });

        let reasoning_effort = if supports_thinking && model_id.id != "grok-4-0709" {
            reasoning_effort.or(Some(ReasoningEffort::High))
        } else {
            None
        };

        let request = CompletionRequest {
            model: model_id.id.clone(),
            messages: xai_messages,
            deferred: None,
            frequency_penalty: None,
            logit_bias: None,
            logprobs: None,
            max_completion_tokens: Some(32768),
            max_tokens: None,
            n: None,
            parallel_tool_calls: None,
            presence_penalty: None,
            reasoning_effort,
            response_format: None,
            search_parameters: None,
            seed: None,
            stop: None,
            stream: Some(true),
            stream_options: Some(StreamOptions {
                include_usage: Some(true),
            }),
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            tool_choice: None,
            tools: xai_tools,
            top_logprobs: None,
            top_p: call_options.as_ref().and_then(|o| o.top_p),
            user: None,
            web_search_options: None,
        };

        let response = self
            .http_client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .map_err(ApiError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| String::new());

            debug!(
                target: "xai::stream",
                "xAI API error - Status: {}, Body: {}",
                status,
                error_text
            );

            return Err(map_http_status_to_api_error(
                self.name(),
                status.as_u16(),
                error_text,
            ));
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(XAIClient::convert_xai_stream(sse_stream, token)))
    }
}

impl XAIClient {
    fn convert_xai_stream(
        mut sse_stream: impl futures::Stream<Item = Result<crate::api::sse::SseEvent, SseParseError>>
        + Unpin
        + Send
        + 'static,
        token: CancellationToken,
    ) -> impl futures::Stream<Item = StreamChunk> + Send + 'static {
        struct ToolCallAccumulator {
            id: String,
            name: String,
            args: String,
        }

        async_stream::stream! {
            let mut content: Vec<AssistantContent> = Vec::new();
            let mut tool_call_indices: Vec<Option<usize>> = Vec::new();
            let mut tool_calls: HashMap<usize, ToolCallAccumulator> = HashMap::new();
            let mut tool_calls_started: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut tool_call_positions: HashMap<usize, usize> = HashMap::new();
            let mut latest_usage: Option<TokenUsage> = None;
            loop {
                if token.is_cancelled() {
                    yield StreamChunk::Error(StreamError::Cancelled);
                    break;
                }

                let event_result = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        yield StreamChunk::Error(StreamError::Cancelled);
                        break;
                    }
                    event = sse_stream.next() => event
                };

                let Some(event_result) = event_result else {
                    break;
                };

                let event = match event_result {
                    Ok(e) => e,
                    Err(e) => {
                        yield StreamChunk::Error(StreamError::SseParse(e));
                        break;
                    }
                };

                if event.data == "[DONE]" {
                    let tool_calls = std::mem::take(&mut tool_calls);
                    let mut final_content = Vec::new();

                    for (block, tool_index) in content.into_iter().zip(tool_call_indices.into_iter())
                    {
                        if let Some(index) = tool_index {
                            let Some(tool_call) = tool_calls.get(&index) else {
                                continue;
                            };
                            if tool_call.id.is_empty() || tool_call.name.is_empty() {
                                debug!(
                                    target: "xai::stream",
                                    "Skipping tool call with missing id/name: id='{}' name='{}'",
                                    tool_call.id,
                                    tool_call.name
                                );
                                continue;
                            }
                            let parameters = serde_json::from_str(&tool_call.args)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            final_content.push(AssistantContent::ToolCall {
                                tool_call: steer_tools::ToolCall {
                                    id: tool_call.id.clone(),
                                    name: tool_call.name.clone(),
                                    parameters,
                                },
                                thought_signature: None,
                            });
                        } else {
                            final_content.push(block);
                        }
                    }

                    yield StreamChunk::MessageComplete(CompletionResponse {
                        content: final_content,
                        usage: latest_usage,
                    });
                    break;
                }

                let chunk: XAIStreamChunk = match serde_json::from_str(&event.data) {
                    Ok(c) => c,
                    Err(e) => {
                        debug!(target: "xai::stream", "Failed to parse chunk: {} data: {}", e, event.data);
                        continue;
                    }
                };

                if let Some(usage) = chunk.usage.as_ref() {
                    latest_usage = Some(map_xai_usage(usage));
                }

                if let Some(choice) = chunk.choices.first() {
                    if let Some(text_delta) = &choice.delta.content {
                        if let Some(AssistantContent::Text { text }) = content.last_mut() { text.push_str(text_delta) } else {
                            content.push(AssistantContent::Text {
                                text: text_delta.clone(),
                            });
                            tool_call_indices.push(None);
                        }
                        yield StreamChunk::TextDelta(text_delta.clone());
                    }

                    if let Some(thinking_delta) = &choice.delta.reasoning_content {
                        if let Some(AssistantContent::Thought {
                                thought: crate::app::conversation::ThoughtContent::Simple { text },
                            }) = content.last_mut() { text.push_str(thinking_delta) } else {
                            content.push(AssistantContent::Thought {
                                thought: crate::app::conversation::ThoughtContent::Simple {
                                    text: thinking_delta.clone(),
                                },
                            });
                            tool_call_indices.push(None);
                        }
                        yield StreamChunk::ThinkingDelta(thinking_delta.clone());
                    }

                    if let Some(tcs) = &choice.delta.tool_calls {
                        for tc in tcs {
                            let entry = tool_calls.entry(tc.index).or_insert_with(|| {
                                ToolCallAccumulator {
                                    id: String::new(),
                                    name: String::new(),
                                    args: String::new(),
                                }
                            });
                            let mut started_now = false;
                            let mut flushed_now = false;

                            if let Some(id) = &tc.id
                                && !id.is_empty() {
                                    entry.id.clone_from(id);
                                }
                            if let Some(func) = &tc.function
                                && let Some(name) = &func.name
                                    && !name.is_empty() {
                                        entry.name.clone_from(name);
                                    }

                            if let std::collections::hash_map::Entry::Vacant(e) = tool_call_positions.entry(tc.index) {
                                let pos = content.len();
                                content.push(AssistantContent::ToolCall {
                                    tool_call: steer_tools::ToolCall {
                                        id: entry.id.clone(),
                                        name: entry.name.clone(),
                                        parameters: serde_json::Value::String(entry.args.clone()),
                                    },
                                    thought_signature: None,
                                });
                                tool_call_indices.push(Some(tc.index));
                                e.insert(pos);
                            }

                            if !entry.id.is_empty()
                                && !entry.name.is_empty()
                                && !tool_calls_started.contains(&tc.index)
                            {
                                tool_calls_started.insert(tc.index);
                                started_now = true;
                                yield StreamChunk::ToolUseStart {
                                    id: entry.id.clone(),
                                    name: entry.name.clone(),
                                };
                            }

                            if let Some(func) = &tc.function
                                && let Some(args) = &func.arguments {
                                    entry.args.push_str(args);
                                    if tool_calls_started.contains(&tc.index) {
                                        if started_now {
                                            if !entry.args.is_empty() {
                                                yield StreamChunk::ToolUseInputDelta {
                                                    id: entry.id.clone(),
                                                    delta: entry.args.clone(),
                                                };
                                                flushed_now = true;
                                            }
                                        } else if !args.is_empty() {
                                            yield StreamChunk::ToolUseInputDelta {
                                                id: entry.id.clone(),
                                                delta: args.clone(),
                                            };
                                        }
                                    }
                                }

                            if started_now && !flushed_now && !entry.args.is_empty() {
                                yield StreamChunk::ToolUseInputDelta {
                                    id: entry.id.clone(),
                                    delta: entry.args.clone(),
                                };
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::provider::StreamChunk;
    use crate::api::sse::SseEvent;
    use crate::app::conversation::{
        AssistantContent, ImageContent, ImageSource, Message, MessageData, UserContent,
    };
    use futures::StreamExt;
    use futures::stream;
    use std::pin::pin;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn test_convert_messages_includes_data_url_image_part() {
        let messages = vec![Message {
            data: MessageData::User {
                content: vec![
                    UserContent::Text {
                        text: "describe".to_string(),
                    },
                    UserContent::Image {
                        image: ImageContent {
                            mime_type: "image/png".to_string(),
                            source: ImageSource::DataUrl {
                                data_url: "data:image/png;base64,Zm9v".to_string(),
                            },
                            width: None,
                            height: None,
                            bytes: None,
                            sha256: None,
                        },
                    },
                ],
            },
            timestamp: 1,
            id: "msg-1".to_string(),
            parent_message_id: None,
        }];

        let converted = XAIClient::convert_messages(messages, None).expect("convert messages");
        assert_eq!(converted.len(), 1);

        match &converted[0] {
            XAIMessage::User { content, .. } => match content {
                XAIUserContent::Parts(parts) => {
                    assert_eq!(parts.len(), 2);
                    assert!(matches!(
                        &parts[0],
                        XAIUserContentPart::Text { text } if text == "describe"
                    ));
                    assert!(matches!(
                        &parts[1],
                        XAIUserContentPart::ImageUrl { image_url }
                            if image_url.url == "data:image/png;base64,Zm9v"
                    ));
                }
                other => panic!("Expected parts content, got {other:?}"),
            },
            other => panic!("Expected user message, got {other:?}"),
        }
    }

    #[test]
    fn test_convert_messages_rejects_session_file_image_source() {
        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Image {
                    image: ImageContent {
                        mime_type: "image/png".to_string(),
                        source: ImageSource::SessionFile {
                            relative_path: "session-1/image.png".to_string(),
                        },
                        width: None,
                        height: None,
                        bytes: None,
                        sha256: None,
                    },
                }],
            },
            timestamp: 1,
            id: "msg-1".to_string(),
            parent_message_id: None,
        }];

        let err =
            XAIClient::convert_messages(messages, None).expect_err("expected unsupported feature");
        match err {
            ApiError::UnsupportedFeature {
                provider,
                feature,
                details,
            } => {
                assert_eq!(provider, "xai");
                assert_eq!(feature, "image input source");
                assert!(details.contains("session file"));
            }
            other => panic!("Expected UnsupportedFeature, got {other:?}"),
        }
    }

    #[test]
    fn test_map_xai_usage() {
        let usage = XAIUsage {
            prompt_tokens: 15,
            completion_tokens: 9,
            total_tokens: 24,
            num_sources_used: None,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };

        assert_eq!(map_xai_usage(&usage), TokenUsage::new(15, 9, 24));
    }

    #[test]
    fn test_non_stream_completion_maps_usage() {
        let usage = XAIUsage {
            prompt_tokens: 6,
            completion_tokens: 4,
            total_tokens: 10,
            num_sources_used: None,
            prompt_tokens_details: None,
            completion_tokens_details: None,
        };
        let choice = Choice {
            index: 0,
            message: AssistantMessage {
                content: Some("hello".to_string()),
                tool_calls: None,
                reasoning_content: None,
            },
            finish_reason: Some("stop".to_string()),
        };
        let response = XAICompletionResponse {
            id: "resp_1".to_string(),
            object: "chat.completion".to_string(),
            created: 1,
            model: "grok-test".to_string(),
            choices: vec![choice],
            usage: Some(usage),
            system_fingerprint: None,
            citations: None,
            debug_output: None,
        };

        let converted = convert_xai_completion_response(response).expect("response should map");

        assert_eq!(converted.usage, Some(TokenUsage::new(6, 4, 10)));
        assert!(matches!(
            converted.content.first(),
            Some(AssistantContent::Text { text }) if text == "hello"
        ));
    }

    #[tokio::test]
    async fn test_convert_xai_stream_captures_final_usage() {
        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-1","choices":[],"usage":{"prompt_tokens":12,"completion_tokens":5,"total_tokens":17}}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: "[DONE]".to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(XAIClient::convert_xai_stream(sse_stream, token));

        let first_delta = stream.next().await.unwrap();
        assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "Hello"));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.usage, Some(TokenUsage::new(12, 5, 17)));
            assert!(matches!(
                response.content.first(),
                Some(AssistantContent::Text { text }) if text == "Hello"
            ));
        } else {
            panic!("Expected MessageComplete");
        }
    }
}
