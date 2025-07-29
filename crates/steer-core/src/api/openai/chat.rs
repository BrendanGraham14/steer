use reqwest::{self, header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::error::ApiError;
use crate::api::provider::CompletionResponse;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, MessageData, ThoughtContent, UserContent,
};
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
            .timeout(std::time::Duration::from_secs(300))
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
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let mut openai_messages = Vec::new();

        if let Some(system_content) = system {
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

        // Determine if we should use reasoning based on call options
        let reasoning_effort = if call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .map(|tc| tc.enabled)
            .unwrap_or(false)
        {
            Some(ReasoningEffort::Medium)
        } else {
            None
        };

        let request = OpenAIRequest {
            model: model_id.1.clone(),  // Use the model ID string
            messages: openai_messages,
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            max_tokens: call_options.as_ref().and_then(|o| o.max_tokens),
            top_p: call_options
                .as_ref()
                .and_then(|o| o.top_p)
                .or(Some(1.0)),
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
                        UserContent::AppCommand { command, response } => {
                            // Format app command and response for the model
                            let mut text = format!("App command: {command:?}");
                            if let Some(resp) = response {
                                text.push_str(&format!("\nResponse: {resp:?}"));
                            }
                            text_parts.push(OpenAIContentPart::Text { text });
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
                        AssistantContent::ToolCall { tool_call } => {
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
                });
            }
        }

        CompletionResponse { content }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::{
        AppCommandType, CommandResponse, Message, MessageData, UserContent,
    };

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
            OpenAIMessage::User { content, .. } => {
                match content {
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
                }
            }
            _ => unreachable!("Expected user message"),
        }
    }

    #[test]
    fn test_convert_message_with_app_command() {
        let client = Client::new("test_key".to_string());

        let message = Message {
            data: MessageData::User {
                content: vec![UserContent::AppCommand {
                    command: AppCommandType::Clear,
                    response: Some(CommandResponse::Text("Available commands...".to_string())),
                }],
            },
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let result = client.convert_message(message).unwrap();
        assert_eq!(result.len(), 1);

        match &result[0] {
            OpenAIMessage::User { content, .. } => match content {
                OpenAIContent::String(text) => {
                    assert!(text.contains("App command: Clear"));
                    assert!(text.contains("Response: Text"));
                }
                _ => unreachable!("Expected string content"),
            },
            _ => unreachable!("Expected user message"),
        }
    }
}
