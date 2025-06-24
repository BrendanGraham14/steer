use anyhow::Result;
use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::provider::{CompletionResponse, Provider};
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, ThoughtContent, ToolResult, UserContent,
};
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
    logit_bias: Option<HashMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logprobs: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
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
    seed: Option<u64>,
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
    choices: Vec<Choice>,
    usage: OpenAIUsage,
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
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
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
    pub fn new(api_key: String) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {}", api_key))
                .expect("Invalid API key format"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout for o3
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http_client: client,
        }
    }

    fn convert_messages(
        &self,
        messages: Vec<AppMessage>,
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
            match message {
                AppMessage::User { content, .. } => {
                    // Convert UserContent to text
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

                    // Only add the message if it has content after filtering
                    if !combined_text.trim().is_empty() {
                        openai_messages.push(OpenAIMessage::User {
                            content: OpenAIContent::String(combined_text),
                            name: None,
                        });
                    }
                }
                AppMessage::Assistant { content, .. } => {
                    // Convert AssistantContent to OpenAI format
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();
                    let mut _reasoning_content = None;

                    for content_block in content {
                        match content_block {
                            AssistantContent::Text { text } => {
                                text_parts.push(text.clone());
                            }
                            AssistantContent::ToolCall { tool_call } => {
                                tool_calls.push(OpenAIToolCall {
                                    id: tool_call.id.clone(),
                                    tool_type: "function".to_string(),
                                    function: OpenAIFunctionCall {
                                        name: tool_call.name.clone(),
                                        arguments: tool_call.parameters.to_string(),
                                    },
                                });
                            }
                            AssistantContent::Thought { thought } => {
                                // For OpenAI models that support reasoning, convert to reasoning_content
                                _reasoning_content = Some(thought.display_text());
                            }
                        }
                    }

                    // Build the assistant message
                    let content = if text_parts.is_empty() {
                        None
                    } else {
                        Some(OpenAIContent::String(text_parts.join("\n")))
                    };

                    let tool_calls_opt = if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    };

                    openai_messages.push(OpenAIMessage::Assistant {
                        content,
                        tool_calls: tool_calls_opt,
                        name: None,
                    });
                }
                AppMessage::Tool {
                    tool_use_id,
                    result,
                    ..
                } => {
                    // Convert ToolResult to OpenAI format
                    let content_text = match result {
                        ToolResult::Success { output } => {
                            if output.trim().is_empty() {
                                "(No output)".to_string()
                            } else {
                                output
                            }
                        }
                        ToolResult::Error { error } => {
                            format!("Error: {}", error)
                        }
                    };

                    openai_messages.push(OpenAIMessage::Tool {
                        content: OpenAIContent::String(content_text),
                        tool_call_id: tool_use_id,
                        name: None,
                    });
                }
            }
        }

        openai_messages
    }

    fn convert_tools(&self, tools: Vec<ToolSchema>) -> Vec<OpenAITool> {
        tools
            .into_iter()
            .map(|tool| OpenAITool {
                tool_type: "function".to_string(),
                function: OpenAIFunction {
                    name: tool.name,
                    description: tool.description,
                    parameters: serde_json::json!({
                        "type": tool.input_schema.schema_type,
                        "properties": tool.input_schema.properties,
                        "required": tool.input_schema.required,
                    }),
                },
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
        messages: Vec<AppMessage>,
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
                max_completion_tokens: None,
                metadata: None,
                modalities: None,
                n: None,
                parallel_tool_calls: None,
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
                temperature: Some(1.0),
                tool_choice: None,
                tools: openai_tools,
                top_logprobs: None,
                top_p: None,
                user: None,
                web_search_options: None,
            }
        };

        let response = self
            .http_client
            .post(API_URL)
            .json(&request)
            .send()
            .await
            .map_err(ApiError::Network)?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| String::new());

            debug!(
                target: "openai::complete",
                "OpenAI API error - Status: {}, Body: {}",
                status,
                error_text
            );

            return match status.as_u16() {
                429 => Err(ApiError::RateLimited {
                    provider: self.name().to_string(),
                    details: error_text,
                }),
                400 => Err(ApiError::InvalidRequest {
                    provider: self.name().to_string(),
                    details: error_text,
                }),
                401 => Err(ApiError::AuthenticationFailed {
                    provider: self.name().to_string(),
                    details: error_text,
                }),
                _ => Err(ApiError::ServerError {
                    provider: self.name().to_string(),
                    status_code: status.as_u16(),
                    details: error_text,
                }),
            };
        }

        let response_text = tokio::select! {
            _ = token.cancelled() => {
                debug!(target: "openai::complete", "Cancellation token triggered while reading successful response body.");
                return Err(ApiError::Cancelled { provider: self.name().to_string() });
            }
            text_res = response.text() => {
                text_res?
            }
        };

        let openai_response: OpenAICompletionResponse = serde_json::from_str(&response_text)
            .map_err(|e| {
                error!(
                    target: "openai::complete",
                    "Failed to parse response: {}, Body: {}",
                    e,
                    response_text
                );
                ApiError::ResponseParsingError {
                    provider: self.name().to_string(),
                    details: format!("Error: {}, Body: {}", e, response_text),
                }
            })?;

        // Convert OpenAI response to our CompletionResponse
        if let Some(choice) = openai_response.choices.first() {
            let mut content_blocks = Vec::new();

            // Add reasoning content if present (convert to thought)
            if let Some(reasoning) = &choice.message.reasoning_content {
                content_blocks.push(AssistantContent::Thought {
                    thought: ThoughtContent::Simple {
                        text: reasoning.clone(),
                    },
                });
            }

            // Add regular content
            if let Some(content) = &choice.message.content {
                if !content.trim().is_empty() {
                    content_blocks.push(AssistantContent::Text {
                        text: content.clone(),
                    });
                }
            }

            // Add tool calls
            if let Some(tool_calls) = &choice.message.tool_calls {
                for tool_call in tool_calls {
                    // Parse the arguments JSON string
                    let parameters = serde_json::from_str(&tool_call.function.arguments)
                        .unwrap_or(serde_json::Value::Null);

                    content_blocks.push(AssistantContent::ToolCall {
                        tool_call: tools::ToolCall {
                            id: tool_call.id.clone(),
                            name: tool_call.function.name.clone(),
                            parameters,
                        },
                    });
                }
            }

            Ok(crate::api::provider::CompletionResponse {
                content: content_blocks,
            })
        } else {
            Err(ApiError::NoChoices {
                provider: self.name().to_string(),
            })
        }
    }
}
