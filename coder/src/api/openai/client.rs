use anyhow::Result;
use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::messages::{ContentBlock, Message, MessageContent, MessageRole};
use crate::api::provider::{CompletionResponse, Provider};
use tools::ToolSchema;

const API_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Clone)]
pub struct OpenAIClient {
    http_client: reqwest::Client,
}

// OpenAI-specific message format
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum OpenAIMessage {
    System {
        content: OpenAIContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    User {
        content: OpenAIContent,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Assistant {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<OpenAIContent>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<OpenAIToolCall>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    Tool {
        content: OpenAIContent,
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
}

// OpenAI content can be a string or an array of content parts
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIContent {
    String(String),
    Array(Vec<OpenAIContentPart>),
}

// OpenAI content parts for multi-modal messages
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
enum OpenAIContentPart {
    #[serde(rename = "text")]
    Text { text: String },
}

// OpenAI function calling format
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

// OpenAI tool format
#[derive(Debug, Serialize, Deserialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String, // "function"
    function: OpenAIFunction,
}

// OpenAI tool call
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String, // JSON string
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ReasoningEffort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ServiceTier {
    Auto,
    Default,
    Flex,
}

#[derive(Debug, Serialize, Deserialize)]
struct AudioOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum StopSequences {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize)]
struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    include_usage: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "required")]
    Required,
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
#[serde(untagged)]
enum ResponseFormat {
    JsonObject {
        #[serde(rename = "type")]
        format_type: String, // "json_object"
    },
    JsonSchema {
        #[serde(rename = "type")]
        format_type: String, // "json_schema"
        json_schema: serde_json::Value,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PredictionType {
    Content,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Prediction {
    Content {
        #[serde(rename = "type")]
        prediction_type: PredictionType,
        content: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct WebSearchOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_results: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<AudioOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    logit_bias: Option<HashMap<String, i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logprobs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modalities: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prediction: Option<Prediction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<ServiceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<StopSequences>,
    #[serde(skip_serializing_if = "Option::is_none")]
    store: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
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
struct OpenAICompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: OpenAIUsage,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIChoice {
    index: usize,
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIResponseMessage {
    role: String,
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PromptTokensDetails {
    cached_tokens: usize,
    audio_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionTokensDetails {
    reasoning_tokens: usize,
    audio_tokens: usize,
    accepted_prediction_tokens: usize,
    rejected_prediction_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_tokens_details: Option<CompletionTokensDetails>,
}

impl OpenAIClient {
    pub fn new(api_key: &str) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "Authorization",
            header::HeaderValue::from_str(&format!("Bearer {}", api_key)).unwrap(),
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

    fn convert_messages(
        &self,
        messages: Vec<Message>,
        system: Option<String>,
    ) -> Vec<OpenAIMessage> {
        let mut openai_messages = Vec::new();

        // Add system message if provided
        if let Some(system_content) = system {
            openai_messages.push(OpenAIMessage::System {
                content: OpenAIContent::String(system_content),
                name: None,
            });
        }

        // Convert our messages to OpenAI format
        for message in messages {
            match message.role {
                MessageRole::User => {
                    // Convert message content
                    let content = match &message.content {
                        MessageContent::Text { content } => OpenAIContent::String(content.clone()),
                        MessageContent::StructuredContent { content } => {
                            // Check if this is actually tool results (shouldn't happen with fixed convert_conversation)
                            // This is a fallback for backwards compatibility
                            let has_tool_results = content
                                .0
                                .iter()
                                .any(|block| matches!(block, ContentBlock::ToolResult { .. }));

                            if has_tool_results {
                                // Convert each ToolResult to a Tool message
                                for block in &content.0 {
                                    if let ContentBlock::ToolResult {
                                        tool_use_id,
                                        content: result_blocks,
                                        is_error,
                                    } = block
                                    {
                                        // Convert result content blocks to string
                                        let result_text = result_blocks
                                            .iter()
                                            .filter_map(|block| match block {
                                                ContentBlock::Text { text } => Some(text.clone()),
                                                _ => None,
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n");

                                        // Add error prefix if this is an error result
                                        let final_content = if is_error.unwrap_or(false) {
                                            format!("Error: {}", result_text)
                                        } else {
                                            result_text
                                        };

                                        openai_messages.push(OpenAIMessage::Tool {
                                            content: OpenAIContent::String(final_content),
                                            tool_call_id: tool_use_id.clone(),
                                            name: None,
                                        });
                                    }
                                }
                                // Skip adding a User message in this case
                                continue;
                            } else {
                                // Regular structured content - convert to text for now
                                let text_parts: Vec<String> = content
                                    .0
                                    .iter()
                                    .filter_map(|block| match block {
                                        ContentBlock::Text { text } => Some(text.clone()),
                                        _ => None,
                                    })
                                    .collect();
                                OpenAIContent::String(text_parts.join("\n"))
                            }
                        }
                    };

                    openai_messages.push(OpenAIMessage::User {
                        content,
                        name: None,
                    });
                }
                MessageRole::Assistant => {
                    // Handle assistant messages which may have tool calls
                    match &message.content {
                        MessageContent::Text { content } => {
                            openai_messages.push(OpenAIMessage::Assistant {
                                content: Some(OpenAIContent::String(content.clone())),
                                tool_calls: None,
                                name: None,
                            });
                        }
                        MessageContent::StructuredContent { content } => {
                            let mut text_content = String::new();
                            let mut tool_calls = Vec::new();

                            for block in &content.0 {
                                match block {
                                    ContentBlock::Text { text } => {
                                        if !text_content.is_empty() {
                                            text_content.push('\n');
                                        }
                                        text_content.push_str(text);
                                    }
                                    ContentBlock::ToolUse { id, name, input } => {
                                        tool_calls.push(OpenAIToolCall {
                                            id: id.clone(),
                                            tool_type: "function".to_string(),
                                            function: OpenAIFunctionCall {
                                                name: name.clone(),
                                                arguments: input.to_string(),
                                            },
                                        });
                                    }
                                    _ => {
                                        // Handle other content blocks as text for simplicity
                                        if !text_content.is_empty() {
                                            text_content.push('\n');
                                        }
                                        text_content.push_str(&format!("{:?}", block));
                                    }
                                }
                            }

                            let final_content = if text_content.is_empty() {
                                None
                            } else {
                                Some(OpenAIContent::String(text_content))
                            };

                            let final_tool_calls = if tool_calls.is_empty() {
                                None
                            } else {
                                Some(tool_calls)
                            };

                            openai_messages.push(OpenAIMessage::Assistant {
                                content: final_content,
                                tool_calls: final_tool_calls,
                                name: None,
                            });
                        }
                    }
                }
                MessageRole::Tool => {
                    // Handle tool messages - extract tool_call_id and content from ToolResult blocks
                    match &message.content {
                        MessageContent::StructuredContent { content } => {
                            // Tool messages contain ToolResult blocks
                            for block in &content.0 {
                                if let ContentBlock::ToolResult {
                                    tool_use_id,
                                    content: result_blocks,
                                    is_error,
                                } = block
                                {
                                    // Convert result content blocks to string
                                    let result_text = result_blocks
                                        .iter()
                                        .filter_map(|block| match block {
                                            ContentBlock::Text { text } => Some(text.clone()),
                                            _ => None,
                                        })
                                        .collect::<Vec<_>>()
                                        .join("\n");

                                    // Add error prefix if this is an error result
                                    let final_content = if is_error.unwrap_or(false) {
                                        format!("Error: {}", result_text)
                                    } else {
                                        result_text
                                    };

                                    openai_messages.push(OpenAIMessage::Tool {
                                        content: OpenAIContent::String(final_content),
                                        tool_call_id: tool_use_id.clone(),
                                        name: None,
                                    });
                                }
                            }
                        }
                        MessageContent::Text { content } => {
                            // Fallback for tool messages with simple text content
                            // This shouldn't normally happen but we'll handle it gracefully
                            warn!(target: "openai::convert_messages", "Tool message has simple text content instead of structured content: {}", content);
                        }
                    }
                }
            }
        }

        openai_messages
    }

    fn convert_tools(&self, tools: Vec<ToolSchema>) -> Vec<OpenAITool> {
        tools
            .into_iter()
            .map(|tool| {
                // Convert our input schema to OpenAI's parameters format
                let parameters = serde_json::json!({
                    "type": tool.input_schema.schema_type,
                    "properties": tool.input_schema.properties,
                    "required": tool.input_schema.required,
                });

                OpenAITool {
                    tool_type: "function".to_string(),
                    function: OpenAIFunction {
                        name: tool.name,
                        description: tool.description,
                        parameters,
                    },
                }
            })
            .collect()
    }
}

#[async_trait]
impl Provider for OpenAIClient {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        // <-- Use ApiError
        let openai_messages = self.convert_messages(messages, system);
        let openai_tools = tools.map(|t| self.convert_tools(t));

        let request = if model.supports_thinking() {
            CompletionRequest {
                model: model.as_ref().to_string(),
                messages: openai_messages,
                audio: None,
                frequency_penalty: None,
                logit_bias: None,
                logprobs: None,
                max_completion_tokens: Some(100_000), // May need to tweak based on context window
                metadata: None,
                modalities: None,
                n: None,
                parallel_tool_calls: None,
                prediction: None,
                presence_penalty: None,
                reasoning_effort: Some(ReasoningEffort::High),
                response_format: None,
                seed: None,
                service_tier: None,
                stop: None,
                store: None,
                stream: None,
                stream_options: None,
                temperature: Some(1.0),
                tool_choice: None,
                tools: openai_tools,
                top_logprobs: None,
                top_p: None,
                user: None,
                web_search_options: None,
            }
        } else {
            CompletionRequest {
                model: model.as_ref().to_string(),
                messages: openai_messages,
                audio: None,
                frequency_penalty: None,
                logit_bias: None,
                logprobs: None,
                max_completion_tokens: Some(32_000),
                metadata: None,
                modalities: None,
                n: None,
                parallel_tool_calls: Some(true),
                prediction: None,
                presence_penalty: None,
                reasoning_effort: None,
                response_format: None,
                seed: None,
                service_tier: None,
                stop: None,
                store: None,
                stream: None,
                stream_options: None,
                temperature: Some(0.7),
                tool_choice: None,
                tools: openai_tools,
                top_logprobs: None,
                top_p: None,
                user: None,
                web_search_options: None,
            }
        };

        debug!(target: "OpenAI API Request", "{:?}", request);

        // Log the full request payload as JSON for detailed debugging
        match serde_json::to_string_pretty(&request) {
            Ok(json_payload) => {
                debug!(target: "Full OpenAI API Request Payload (JSON)", "{}", json_payload);
            }
            Err(e) => {
                error!(target: "OpenAI API Request Serialization Error", "Failed to serialize request to JSON: {}", e);
            }
        }

        let request_builder = self.http_client.post(API_URL).json(&request);

        // Race the request sending against cancellation
        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "openai::complete", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string() });
            }
            res = request_builder.send() => {
                res.map_err(ApiError::Network)?
            }
        };

        // Check for cancellation before processing status
        if token.is_cancelled() {
            debug!(target: "openai::complete", "Cancellation token triggered after sending request, before status check.");
            return Err(ApiError::Cancelled {
                provider: self.name().to_string(),
            });
        }

        let status = response.status(); // Store status before consuming response
        if !status.is_success() {
            // Race reading the error text against cancellation
            let error_text = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    debug!(target: "openai::complete", "Cancellation token triggered while reading error response body.");
                    return Err(ApiError::Cancelled{ provider: self.name().to_string() });
                }
                text_res = response.text() => {
                    text_res.map_err(ApiError::Network)?
                }
            };
            // Map status codes to ApiError variants
            return Err(match status.as_u16() {
                401 => ApiError::AuthenticationFailed {
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

        // Race reading the successful response text against cancellation
        let response_text = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "openai::complete", "Cancellation token triggered while reading successful response body.");
                return Err(ApiError::Cancelled{ provider: self.name().to_string() });
            }
            text_res = response.text() => {
                 text_res.map_err(ApiError::Network)?
            }
        };

        // Parse the text into the OpenAICompletionResponse
        let openai_completion: OpenAICompletionResponse = serde_json::from_str(&response_text)
            .map_err(|e| ApiError::ResponseParsingError {
                provider: self.name().to_string(),
                details: format!("Error: {}, Body: {}", e, response_text),
            })?;

        if openai_completion.choices.is_empty() {
            return Err(ApiError::NoChoices {
                provider: self.name().to_string(),
            });
        }

        let choice = &openai_completion.choices[0];
        let message = &choice.message;

        let mut content_blocks = Vec::new();

        if let Some(text) = &message.content {
            if !text.is_empty() {
                content_blocks.push(ContentBlock::Text { text: text.clone() });
            }
        }

        if let Some(tool_calls) = &message.tool_calls {
            for tool_call in tool_calls {
                // Parse the arguments JSON string into a Value
                let input = match serde_json::from_str::<serde_json::Value>(
                    &tool_call.function.arguments,
                ) {
                    Ok(value) => value,
                    Err(e) => {
                        error!(target: "openai::complete", "Failed to parse tool call arguments as JSON: {}. Raw: {}", e, tool_call.function.arguments);
                        serde_json::Value::Null
                    }
                };

                content_blocks.push(ContentBlock::ToolUse {
                    id: tool_call.id.clone(),
                    name: tool_call.function.name.clone(),
                    input,
                });
            }
        }

        let completion = CompletionResponse {
            content: content_blocks,
        };

        Ok(completion)
    }
}
