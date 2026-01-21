use futures::StreamExt;
use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::error::{ApiError, StreamError};
use crate::api::provider::{CompletionResponse, CompletionStream, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, MessageData, ThoughtContent, UserContent,
};
use crate::app::SystemContext;
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::ToolSchema;

use super::types::{OpenAIFunction, OpenAITool, ServiceTier, ToolChoice};

#[allow(dead_code)]
const DEFAULT_API_URL: &str = "https://api.openai.com/v1/chat/completions";

#[derive(Clone)]
#[allow(dead_code)]
pub(super) struct Client {
    http_client: reqwest::Client,
    base_url: String,
}

#[allow(dead_code)]
impl Client {
    pub(super) fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, None)
    }

    pub(super) fn with_base_url(api_key: String, base_url: Option<String>) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&format!("Bearer {api_key}"))
                .expect("Invalid API key format"),
        );

        let http_client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(super::HTTP_TIMEOUT_SECS))
            .build()
            .expect("Failed to build HTTP client");

        let base_url = crate::api::util::normalize_chat_url(base_url.as_deref(), DEFAULT_API_URL);

        Self {
            http_client,
            base_url,
        }
    }

    pub(super) async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let mut openai_messages = Vec::new();

        let system_text = system.and_then(|context| context.render());
        if let Some(system_content) = system_text {
            openai_messages.push(OpenAIMessage::System {
                content: OpenAIContent::String(system_content),
                name: None,
            });
        }

        for message in messages {
            openai_messages.extend(self.convert_message(message)?);
        }

        let openai_tools = tools.map(|tools| {
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
                            "required": tool.input_schema.required
                        }),
                    },
                })
                .collect()
        });

        // Determine reasoning effort from call options (catalog or per-call)
        let reasoning_effort = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .and_then(|tc| {
                if !tc.enabled {
                    return None;
                }
                match tc
                    .effort
                    .unwrap_or(crate::config::toml_types::ThinkingEffort::Medium)
                {
                    crate::config::toml_types::ThinkingEffort::Low => Some(ReasoningEffort::Low),
                    crate::config::toml_types::ThinkingEffort::Medium => {
                        Some(ReasoningEffort::Medium)
                    }
                    crate::config::toml_types::ThinkingEffort::High => Some(ReasoningEffort::High),
                    crate::config::toml_types::ThinkingEffort::XHigh => {
                        Some(ReasoningEffort::XHigh)
                    }
                }
            });

        let request = OpenAIRequest {
            model: model_id.id.clone(), // Use the model ID string
            messages: openai_messages,
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            max_tokens: call_options.as_ref().and_then(|o| o.max_tokens),
            top_p: call_options.as_ref().and_then(|o| o.top_p).or(Some(1.0)),
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            stream: Some(false),
            n: None,
            logit_bias: None,
            tools: openai_tools,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            reasoning_effort,
            audio: None,
            stream_options: None,
            service_tier: None,
            user: None,
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
            let body = response.text().await.unwrap_or_default();
            debug!(
                target: "openai::chat",
                "API error status={} body={}", status, body
            );
            return Err(ApiError::ServerError {
                provider: super::PROVIDER_NAME.to_string(),
                status_code: status.as_u16(),
                details: body,
            });
        }

        let body_text = tokio::select! {
            _ = token.cancelled() => {
                return Err(ApiError::Cancelled { provider: super::PROVIDER_NAME.to_string() });
            }
            text = response.text() => {
                text?
            }
        };

        let parsed: OpenAIResponse = serde_json::from_str(&body_text).map_err(|e| {
            error!(
                target: "openai::chat",
                "Failed to parse OpenAI response: {} body={}", e, body_text
            );
            ApiError::ResponseParsingError {
                provider: super::PROVIDER_NAME.to_string(),
                details: e.to_string(),
            }
        })?;

        if let Some(choice) = parsed.choices.first() {
            Ok(self.convert_response_message(&choice.message))
        } else {
            Err(ApiError::ResponseParsingError {
                provider: super::PROVIDER_NAME.to_string(),
                details: "No choices in response".to_string(),
            })
        }
    }

    fn convert_message(&self, message: AppMessage) -> Result<Vec<OpenAIMessage>, ApiError> {
        match message.data {
            MessageData::User { content, .. } => {
                let mut text_parts = Vec::new();
                for c in content {
                    match c {
                        UserContent::Text { text } => {
                            text_parts.push(OpenAIContentPart::Text { text });
                        }
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => {
                            let formatted = UserContent::format_command_execution_as_xml(
                                &command, &stdout, &stderr, exit_code,
                            );
                            text_parts.push(OpenAIContentPart::Text { text: formatted });
                        }
                    }
                }
                Ok(vec![OpenAIMessage::User {
                    content: if text_parts.len() == 1 {
                        let OpenAIContentPart::Text { text } = &text_parts[0];
                        OpenAIContent::String(text.clone())
                    } else {
                        OpenAIContent::Array(text_parts)
                    },
                    name: None,
                }])
            }
            MessageData::Assistant { content, .. } => {
                let mut tool_calls = Vec::new();
                let mut text_content = String::new();

                for c in content {
                    match c {
                        AssistantContent::Text { text } => {
                            if !text_content.is_empty() {
                                text_content.push('\n');
                            }
                            text_content.push_str(&text);
                        }
                        AssistantContent::ToolCall { tool_call, .. } => {
                            tool_calls.push(OpenAIToolCall {
                                id: tool_call.id.clone(),
                                tool_type: "function".to_string(),
                                function: OpenAIFunctionCall {
                                    name: tool_call.name.clone(),
                                    arguments: serde_json::to_string(&tool_call.parameters)
                                        .unwrap_or_default(),
                                },
                            });
                        }
                        AssistantContent::Thought { .. } => {
                            // Skip thoughts - don't include in API request
                        }
                    }
                }

                // Don't include thinking in messages sent to API
                let content = if text_content.is_empty() {
                    None
                } else {
                    Some(OpenAIContent::String(text_content))
                };

                Ok(vec![OpenAIMessage::Assistant {
                    content,
                    tool_calls: if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    },
                    name: None,
                }])
            }
            MessageData::Tool {
                tool_use_id,
                result,
                ..
            } => {
                // Convert tool result to function message
                let content = result.llm_format();

                Ok(vec![OpenAIMessage::Tool {
                    content: OpenAIContent::String(content),
                    tool_call_id: tool_use_id.clone(),
                    name: None,
                }])
            }
        }
    }

    fn convert_response_message(&self, message: &OpenAIResponseMessage) -> CompletionResponse {
        let mut content = Vec::new();

        if let Some(msg_content) = &message.content {
            match msg_content {
                OpenAIContent::String(text) => {
                    content.push(AssistantContent::Text { text: text.clone() });
                }
                OpenAIContent::Array(parts) => {
                    for part in parts {
                        match part {
                            OpenAIContentPart::Text { text } => {
                                content.push(AssistantContent::Text { text: text.clone() });
                            }
                        }
                    }
                }
            }
        }

        if let Some(reasoning_content) = &message.reasoning_content {
            content.push(AssistantContent::Thought {
                thought: ThoughtContent::Simple {
                    text: reasoning_content.clone(),
                },
            });
        }

        if let Some(tool_calls) = &message.tool_calls {
            for tc in tool_calls {
                let arguments: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                content.push(AssistantContent::ToolCall {
                    tool_call: steer_tools::ToolCall {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        parameters: arguments,
                    },
                    thought_signature: None,
                });
            }
        }

        CompletionResponse { content }
    }

    pub(super) async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let mut openai_messages = Vec::new();

        let system_text = system.and_then(|context| context.render());
        if let Some(system_content) = system_text {
            openai_messages.push(OpenAIMessage::System {
                content: OpenAIContent::String(system_content),
                name: None,
            });
        }

        for message in messages {
            openai_messages.extend(self.convert_message(message)?);
        }

        let openai_tools = tools.map(|tools| {
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
                            "required": tool.input_schema.required
                        }),
                    },
                })
                .collect()
        });

        let reasoning_effort = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .and_then(|tc| {
                if !tc.enabled {
                    return None;
                }
                match tc
                    .effort
                    .unwrap_or(crate::config::toml_types::ThinkingEffort::Medium)
                {
                    crate::config::toml_types::ThinkingEffort::Low => Some(ReasoningEffort::Low),
                    crate::config::toml_types::ThinkingEffort::Medium => {
                        Some(ReasoningEffort::Medium)
                    }
                    crate::config::toml_types::ThinkingEffort::High => Some(ReasoningEffort::High),
                    crate::config::toml_types::ThinkingEffort::XHigh => {
                        Some(ReasoningEffort::XHigh)
                    }
                }
            });

        let request = OpenAIRequest {
            model: model_id.id.clone(),
            messages: openai_messages,
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            max_tokens: call_options.as_ref().and_then(|o| o.max_tokens),
            top_p: call_options.as_ref().and_then(|o| o.top_p).or(Some(1.0)),
            frequency_penalty: None,
            presence_penalty: None,
            stop: None,
            stream: Some(true),
            n: None,
            logit_bias: None,
            tools: openai_tools,
            tool_choice: None,
            parallel_tool_calls: None,
            response_format: None,
            reasoning_effort,
            audio: None,
            stream_options: None,
            service_tier: None,
            user: None,
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
            let body = response.text().await.unwrap_or_default();
            debug!(
                target: "openai::chat::stream",
                "API error status={} body={}", status, body
            );
            return Err(ApiError::ServerError {
                provider: super::PROVIDER_NAME.to_string(),
                status_code: status.as_u16(),
                details: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(Self::convert_openai_stream(sse_stream, token)))
    }

    fn convert_openai_stream(
        mut sse_stream: impl futures::Stream<Item = Result<crate::api::sse::SseEvent, ApiError>>
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
            loop {
                if token.is_cancelled() {
                    yield StreamChunk::Error(StreamError::Cancelled);
                    break;
                }

                let event_result = tokio::select! {
                    biased;
                    _ = token.cancelled() => {
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
                        yield StreamChunk::Error(StreamError::SseParse(e.to_string()));
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
                                    target: "openai::chat::stream",
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

                    yield StreamChunk::MessageComplete(CompletionResponse { content: final_content });
                    break;
                }

                let chunk: OpenAIStreamChunk = match serde_json::from_str(&event.data) {
                    Ok(c) => c,
                    Err(e) => {
                        debug!(target: "openai::chat::stream", "Failed to parse chunk: {} data: {}", e, event.data);
                        continue;
                    }
                };

                if let Some(choice) = chunk.choices.first() {
                    if let Some(text_delta) = &choice.delta.content {
                        match content.last_mut() {
                            Some(AssistantContent::Text { text }) => text.push_str(text_delta),
                            _ => {
                                content.push(AssistantContent::Text {
                                    text: text_delta.clone(),
                                });
                                tool_call_indices.push(None);
                            }
                        }
                        yield StreamChunk::TextDelta(text_delta.clone());
                    }

                    if let Some(thinking_delta) = &choice.delta.reasoning_content {
                        match content.last_mut() {
                            Some(AssistantContent::Thought {
                                thought: ThoughtContent::Simple { text },
                            }) => text.push_str(thinking_delta),
                            _ => {
                                content.push(AssistantContent::Thought {
                                    thought: ThoughtContent::Simple {
                                        text: thinking_delta.clone(),
                                    },
                                });
                                tool_call_indices.push(None);
                            }
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

                            if let Some(id) = &tc.id {
                                if !id.is_empty() {
                                    entry.id = id.clone();
                                }
                            }
                            if let Some(func) = &tc.function {
                                if let Some(name) = &func.name {
                                    if !name.is_empty() {
                                        entry.name = name.clone();
                                    }
                                }
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

                            if let Some(func) = &tc.function {
                                if let Some(args) = &func.arguments {
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

// OpenAI tool call
#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIFunctionCall,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAIFunctionCall {
    pub name: String,
    pub arguments: String, // JSON string
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    XHigh,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AudioOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StopSequences {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_usage: Option<bool>,
}

// Request/Response types
#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<StopSequences>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    n: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    logit_bias: Option<HashMap<String, f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<AudioOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service_tier: Option<ServiceTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<OpenAIChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIChoice {
    index: u32,
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIResponseMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OpenAIContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    object: String,
    choices: Vec<OpenAIStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    #[allow(dead_code)]
    index: u32,
    delta: OpenAIStreamDelta,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIStreamToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamToolCall {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function: Option<OpenAIStreamFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::sse::SseEvent;
    use crate::app::conversation::{Message, MessageData, UserContent};
    use futures::stream;
    use std::pin::pin;

    #[tokio::test]
    async fn test_convert_openai_stream_text_deltas() {
        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#.to_string(),
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
        let mut stream = pin!(Client::convert_openai_stream(sse_stream, token));

        let first_delta = stream.next().await.unwrap();
        assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "Hello"));

        let second_delta = stream.next().await.unwrap();
        assert!(matches!(second_delta, StreamChunk::TextDelta(ref t) if t == " world"));

        let complete = stream.next().await.unwrap();
        assert!(matches!(complete, StreamChunk::MessageComplete(_)));
    }

    #[tokio::test]
    async fn test_convert_openai_stream_with_reasoning() {
        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"reasoning_content":"Let me think..."},"finish_reason":null}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"The answer is 42"},"finish_reason":null}]}"#.to_string(),
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
        let mut stream = pin!(Client::convert_openai_stream(sse_stream, token));

        let thinking_delta = stream.next().await.unwrap();
        assert!(
            matches!(thinking_delta, StreamChunk::ThinkingDelta(ref t) if t == "Let me think...")
        );

        let text_delta = stream.next().await.unwrap();
        assert!(matches!(text_delta, StreamChunk::TextDelta(ref t) if t == "The answer is 42"));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 2);
            assert!(matches!(
                &response.content[0],
                AssistantContent::Thought { .. }
            ));
            assert!(
                matches!(&response.content[1], AssistantContent::Text { text } if text == "The answer is 42")
            );
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_openai_stream_with_tool_calls() {
        let events = vec![
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"get_weather","arguments":""}}]},"finish_reason":null}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"loc"}}]},"finish_reason":null}]}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: None,
                data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ation\":\"NYC\"}"}}]},"finish_reason":null}]}"#.to_string(),
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
        let mut stream = pin!(Client::convert_openai_stream(sse_stream, token));

        let tool_start = stream.next().await.unwrap();
        assert!(
            matches!(tool_start, StreamChunk::ToolUseStart { ref id, ref name } if id == "call_abc" && name == "get_weather")
        );

        let arg_delta_1 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_1, StreamChunk::ToolUseInputDelta { ref delta, .. } if delta == "{\"loc")
        );

        let arg_delta_2 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_2, StreamChunk::ToolUseInputDelta { ref delta, .. } if delta == "ation\":\"NYC\"}")
        );

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            if let AssistantContent::ToolCall { tool_call, .. } = &response.content[0] {
                assert_eq!(tool_call.name, "get_weather");
                assert_eq!(tool_call.id, "call_abc");
            } else {
                panic!("Expected ToolCall");
            }
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_openai_stream_cancellation() {
        let events = vec![Ok(SseEvent {
            event_type: None,
            data: r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#.to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        token.cancel();

        let mut stream = pin!(Client::convert_openai_stream(sse_stream, token));

        let cancelled = stream.next().await.unwrap();
        assert!(matches!(
            cancelled,
            StreamChunk::Error(StreamError::Cancelled)
        ));
    }

    #[test]
    fn test_convert_message_with_command_execution() {
        let client = Client::new("test_key".to_string());

        let message = Message {
            data: MessageData::User {
                content: vec![
                    UserContent::Text {
                        text: "Here's the result:".to_string(),
                    },
                    UserContent::CommandExecution {
                        command: "ls -la".to_string(),
                        stdout: "total 24\ndrwxr-xr-x  3 user  staff   96 Jan  1 12:00 ."
                            .to_string(),
                        stderr: "".to_string(),
                        exit_code: 0,
                    },
                ],
            },
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let result = client.convert_message(message).unwrap();
        assert_eq!(result.len(), 1);

        match &result[0] {
            OpenAIMessage::User { content, .. } => match content {
                OpenAIContent::Array(parts) => {
                    assert_eq!(parts.len(), 2);

                    // Check first part is plain text
                    match &parts[0] {
                        OpenAIContentPart::Text { text } => {
                            assert_eq!(text, "Here's the result:");
                        }
                    }

                    // Check second part contains command execution
                    match &parts[1] {
                        OpenAIContentPart::Text { text } => {
                            assert!(text.contains("<executed_command>"));
                            assert!(text.contains("ls -la"));
                            assert!(text.contains("total 24"));
                        }
                    }
                }
                _ => unreachable!("Expected array content"),
            },
            _ => unreachable!("Expected user message"),
        }
    }

    #[tokio::test]
    #[ignore = "Requires OPENAI_API_KEY environment variable"]
    async fn test_stream_complete_real_api() {
        dotenvy::dotenv().ok();
        let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
        let client = Client::new(api_key);

        let message = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Say exactly: Hello".to_string(),
                }],
            },
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            id: "test-msg".to_string(),
            parent_message_id: None,
        };

        let model_id = ModelId::new(crate::config::provider::openai(), "gpt-4.1-mini-2025-04-14");
        let token = CancellationToken::new();

        let mut stream = client
            .stream_complete(&model_id, vec![message], None, None, None, token)
            .await
            .expect("stream_complete should succeed");

        let mut got_text_delta = false;
        let mut got_message_complete = false;
        let mut accumulated_text = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                StreamChunk::TextDelta(text) => {
                    got_text_delta = true;
                    accumulated_text.push_str(&text);
                }
                StreamChunk::MessageComplete(response) => {
                    got_message_complete = true;
                    assert!(!response.content.is_empty());
                }
                StreamChunk::Error(e) => panic!("Unexpected error: {e:?}"),
                _ => {}
            }
        }

        assert!(got_text_delta, "Should receive at least one TextDelta");
        assert!(
            got_message_complete,
            "Should receive MessageComplete at the end"
        );
        assert!(
            accumulated_text.to_lowercase().contains("hello"),
            "Response should contain 'hello', got: {accumulated_text}"
        );
    }
}
