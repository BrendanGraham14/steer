use anyhow::Result;
use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::messages::{ContentBlock, Message, MessageContent, MessageRole};
use crate::api::provider::{CompletionResponse, Provider};
use crate::api::tools::Tool;

const API_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Clone)]
pub struct OpenAIClient {
    http_client: reqwest::Client,
}

// OpenAI-specific message format
#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: OpenAIContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
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
struct CompletionRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
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
            openai_messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: OpenAIContent::String(system_content),
                name: None,
            });
        }

        // Convert our messages to OpenAI format
        for message in messages {
            match message.role {
                MessageRole::User | MessageRole::Assistant => {
                    // Convert message content
                    let content = match &message.content {
                        MessageContent::Text { content } => OpenAIContent::String(content.clone()),
                        MessageContent::StructuredContent { content } => {
                            // This is more complex for OpenAI and would need proper handling
                            // For simplicity, we're just converting to a string description
                            OpenAIContent::String(format!("{:?}", content))
                        }
                    };

                    openai_messages.push(OpenAIMessage {
                        role: message.role.to_string(),
                        content,
                        name: None,
                    });
                }
                // Skip other roles or handle them differently
                _ => {
                    warn!(target: "openai::convert_messages", "Skipping message with unsupported role: {}", message.role);
                }
            }
        }

        openai_messages
    }

    fn convert_tools(&self, tools: Vec<Tool>) -> Vec<OpenAITool> {
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
        tools: Option<Vec<Tool>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        // <-- Use ApiError
        let openai_messages = self.convert_messages(messages, system);
        let openai_tools = tools.map(|t| self.convert_tools(t));

        let request = CompletionRequest {
            model: model.as_ref().to_string(),
            messages: openai_messages,
            tools: openai_tools,
            temperature: Some(0.7),
            top_p: None,
            stream: None,
            max_tokens: Some(4000),
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
