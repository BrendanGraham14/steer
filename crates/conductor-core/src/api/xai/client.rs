use async_trait::async_trait;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::provider::{CompletionResponse, Provider};
use crate::api::{Model, error::ApiError};
use crate::app::conversation::{AssistantContent, Message as AppMessage, ToolResult, UserContent};
use conductor_tools::ToolSchema;

const API_URL: &str = "https://api.x.ai/v1/chat/completions";

#[derive(Clone)]
pub struct XAIClient {
    http_client: reqwest::Client,
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
        content: String,
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
    cached_tokens: usize,
    audio_tokens: usize,
    image_tokens: usize,
    text_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletionTokensDetails {
    reasoning_tokens: usize,
    audio_tokens: usize,
    accepted_prediction_tokens: usize,
    rejected_prediction_tokens: usize,
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

impl XAIClient {
    pub fn new(api_key: String) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {api_key}"))
                .expect("Invalid API key format"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout
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
    ) -> Vec<XAIMessage> {
        let mut xai_messages = Vec::new();

        // Add system message if provided
        if let Some(system_content) = system {
            xai_messages.push(XAIMessage::System {
                content: system_content,
                name: None,
            });
        }

        // Convert our messages to xAI format
        for message in messages {
            match &message.data {
                crate::app::conversation::MessageData::User { content, .. } => {
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
                        xai_messages.push(XAIMessage::User {
                            content: combined_text,
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
                            AssistantContent::ToolCall { tool_call } => {
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
                                continue;
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
                    let content_text = match result {
                        ToolResult::Error(e) => format!("Error: {e}"),
                        _ => {
                            let text = result.llm_format();
                            if text.trim().is_empty() {
                                "(No output)".to_string()
                            } else {
                                text
                            }
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

        xai_messages
    }

    fn convert_tools(&self, tools: Vec<ToolSchema>) -> Vec<XAITool> {
        tools
            .into_iter()
            .map(|tool| XAITool {
                tool_type: "function".to_string(),
                function: XAIFunction {
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
impl Provider for XAIClient {
    fn name(&self) -> &'static str {
        "xai"
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let xai_messages = self.convert_messages(messages, system);
        let xai_tools = tools.map(|t| self.convert_tools(t));

        // grok-4 supports thinking by default but not the reasoning_effort parameter
        let reasoning_effort = if model.supports_thinking() && !matches!(model, Model::Grok4_0709) {
            Some(ReasoningEffort::High)
        } else {
            None
        };

        let request = CompletionRequest {
            model: model.as_ref().to_string(),
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
            temperature: Some(1.0),
            tool_choice: None,
            tools: xai_tools,
            top_logprobs: None,
            top_p: None,
            user: None,
            web_search_options: None,
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
                target: "grok::complete",
                "Grok API error - Status: {}, Body: {}",
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

        // Convert xAI response to our CompletionResponse
        if let Some(choice) = xai_response.choices.first() {
            let mut content_blocks = Vec::new();

            // Add reasoning content (thinking) first if present
            if let Some(reasoning) = &choice.message.reasoning_content {
                if !reasoning.trim().is_empty() {
                    content_blocks.push(AssistantContent::Thought {
                        thought: crate::app::conversation::ThoughtContent::Simple {
                            text: reasoning.clone(),
                        },
                    });
                }
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
                        tool_call: conductor_tools::ToolCall {
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
