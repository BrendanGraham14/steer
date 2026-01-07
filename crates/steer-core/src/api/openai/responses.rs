use futures::StreamExt;
use reqwest::{self, header};
use serde_json;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::error::{ApiError, StreamError};
use crate::api::openai::responses_types::{
    InputContentPart, InputItem, InputType, MessageContentPart, ReasoningConfig, ReasoningSummary,
    ReasoningSummaryPart, ResponseOutputItem, ResponsesApiResponse, ResponsesFunctionTool,
    ResponsesRequest, ResponsesToolChoice,
};
use crate::api::provider::{CompletionResponse, CompletionStream, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, MessageData, ThoughtContent, UserContent,
};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::ToolSchema;

const DEFAULT_API_URL: &str = "https://api.openai.com/v1/responses";

#[derive(Clone)]
pub(super) struct Client {
    http_client: reqwest::Client,
    base_url: String,
}

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

        let base_url =
            crate::api::util::normalize_responses_url(base_url.as_deref(), DEFAULT_API_URL);

        Self {
            http_client,
            base_url,
        }
    }

    /// Build a request with proper message structure and tool support
    fn build_request(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
    ) -> ResponsesRequest {
        let input = self.convert_messages_to_input(&messages);

        let responses_tools = tools.map(|tools| {
            tools
                .into_iter()
                .map(|tool| ResponsesFunctionTool {
                    tool_type: "function".to_string(),
                    name: tool.name,
                    description: Some(tool.description),
                    parameters: serde_json::json!({
                        "type": tool.input_schema.schema_type,
                        "properties": tool.input_schema.properties,
                        "required": tool.input_schema.required,
                        "additionalProperties": false
                    }),
                    strict: false,
                })
                .collect()
        });

        // Configure reasoning for supported models based on call options
        let reasoning = call_options
            .as_ref()
            .and_then(|opts| opts.thinking_config.as_ref())
            .and_then(|tc| {
                if !tc.enabled {
                    return None;
                }
                let effort = match tc
                    .effort
                    .unwrap_or(crate::config::toml_types::ThinkingEffort::Medium)
                {
                    crate::config::toml_types::ThinkingEffort::Low => {
                        Some(crate::api::openai::responses_types::ReasoningEffort::Low)
                    }
                    crate::config::toml_types::ThinkingEffort::Medium => {
                        Some(crate::api::openai::responses_types::ReasoningEffort::Medium)
                    }
                    crate::config::toml_types::ThinkingEffort::High => {
                        Some(crate::api::openai::responses_types::ReasoningEffort::High)
                    }
                };
                Some(ReasoningConfig {
                    effort,
                    summary: Some(ReasoningSummary::Detailed),
                })
            });

        let tool_choice = if responses_tools.is_some() {
            Some(ResponsesToolChoice::Auto)
        } else {
            None
        };

        ResponsesRequest {
            model: model_id.1.clone(), // Use the model ID string
            input,
            instructions: system,
            previous_response_id: None,
            temperature: call_options
                .as_ref()
                .and_then(|o| o.temperature)
                .or(Some(1.0)),
            max_output_tokens: call_options.as_ref().and_then(|o| o.max_tokens),
            max_tool_calls: None,
            parallel_tool_calls: Some(true),
            store: Some(false),
            stream: None,
            tools: responses_tools,
            tool_choice,
            metadata: None,
            service_tier: None,
            prompt: None,
            reasoning,
            text: None,
            extra: Default::default(),
        }
    }

    pub(super) fn convert_output(
        &self,
        output: Option<Vec<ResponseOutputItem>>,
    ) -> Vec<AssistantContent> {
        let mut result = Vec::new();
        if let Some(items) = output {
            for item in items {
                match item {
                    ResponseOutputItem::Message { content, .. } => {
                        for part in content {
                            match part {
                                MessageContentPart::OutputText { text, .. } => {
                                    result.push(AssistantContent::Text { text });
                                }
                                MessageContentPart::Other => {}
                            }
                        }
                    }
                    ResponseOutputItem::Reasoning { summary, .. } => {
                        // Extract reasoning text from summary parts
                        let mut reasoning_text = String::new();
                        for part in summary {
                            if let ReasoningSummaryPart::SummaryText { text } = part {
                                if !reasoning_text.is_empty() {
                                    reasoning_text.push('\n');
                                }
                                reasoning_text.push_str(&text);
                            }
                        }
                        if !reasoning_text.is_empty() {
                            result.push(AssistantContent::Thought {
                                thought: ThoughtContent::Simple {
                                    text: reasoning_text,
                                },
                            });
                        }
                    }
                    ResponseOutputItem::FunctionCall {
                        id: _,
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        let parameters = match serde_json::from_str(&arguments) {
                            Ok(params) => params,
                            Err(e) => {
                                tracing::warn!(
                                    target: "openai::responses",
                                    "Failed to parse function call arguments for '{}': {}. Raw arguments: {}",
                                    name,
                                    e,
                                    arguments
                                );
                                // Default to empty object to maintain compatibility
                                serde_json::Value::Object(serde_json::Map::new())
                            }
                        };

                        result.push(AssistantContent::ToolCall {
                            tool_call: steer_tools::ToolCall {
                                id: call_id,
                                name,
                                parameters,
                            },
                        });
                    }
                    ResponseOutputItem::WebSearchCall { .. }
                    | ResponseOutputItem::FileSearchCall { .. }
                    | ResponseOutputItem::Other => {
                        // These are built-in tools that we don't handle yet
                    }
                }
            }
        }
        result
    }

    /// Convert messages to the structured input format that preserves roles
    pub(super) fn convert_messages_to_input(&self, messages: &[AppMessage]) -> Option<InputType> {
        let mut input_items = Vec::new();

        for message in messages {
            match &message.data {
                MessageData::User { content, .. } => {
                    let mut content_parts = Vec::new();

                    for item in content {
                        match item {
                            UserContent::Text { text } => {
                                content_parts
                                    .push(InputContentPart::InputText { text: text.clone() });
                            }
                        }
                    }

                    if !content_parts.is_empty() {
                        input_items.push(InputItem::Message {
                            role: "user".to_string(),
                            content: content_parts,
                        });
                    }
                }
                MessageData::Assistant { content, .. } => {
                    let mut function_calls = Vec::new();
                    let mut content_parts = Vec::new();

                    for item in content {
                        match item {
                            AssistantContent::Text { text } => {
                                content_parts.push(InputContentPart::OutputText {
                                    text: text.clone(),
                                    annotations: vec![],
                                });
                            }
                            AssistantContent::ToolCall { tool_call } => {
                                // Add as a proper function_call item
                                function_calls.push(InputItem::FunctionCall {
                                    item_type: "function_call".to_string(),
                                    call_id: tool_call.id.clone(),
                                    name: tool_call.name.clone(),
                                    arguments: tool_call.parameters.to_string(),
                                });
                            }
                            AssistantContent::Thought { .. } => {
                                // Skip thoughts - they're internal reasoning
                            }
                        }
                    }

                    // Add message content if any
                    if !content_parts.is_empty() {
                        input_items.push(InputItem::Message {
                            role: "assistant".to_string(),
                            content: content_parts,
                        });
                    }

                    // Add function calls as separate items
                    input_items.extend(function_calls);
                }
                MessageData::Tool {
                    tool_use_id,
                    result,
                    ..
                } => {
                    // Tool results should be included as function call outputs
                    let content_text = result.llm_format();
                    input_items.push(InputItem::FunctionCallOutput {
                        item_type: "function_call_output".to_string(),
                        call_id: tool_use_id.clone(),
                        output: content_text,
                    });
                }
            }
        }

        if input_items.is_empty() {
            None
        } else {
            Some(InputType::Messages(input_items))
        }
    }

    /// Convert a Responses API response to an app CompletionResponse
    pub(super) fn convert_response(&self, response: ResponsesApiResponse) -> CompletionResponse {
        let content = self.convert_output(response.output);

        // Note: The top-level `reasoning` field in the response is just metadata
        // about reasoning configuration. The actual reasoning content comes through
        // as a ResponseOutputItem::Reasoning in the output array, which is handled
        // in convert_output above.

        // Check for reasoning tokens in usage to verify reasoning happened
        if let Some(usage) = response.usage {
            if let Some(details) = usage.output_tokens_details {
                if let Some(reasoning_tokens) = details.reasoning_tokens {
                    if reasoning_tokens > 0
                        && !content
                            .iter()
                            .any(|c| matches!(c, AssistantContent::Thought { .. }))
                    {
                        // Reasoning happened but wasn't included in the output
                        debug!(
                            target: "openai::responses",
                            "Model used {} reasoning tokens but no reasoning output provided",
                            reasoning_tokens
                        );
                    }
                }
            }
        }

        CompletionResponse { content }
    }
}

impl Client {
    pub(super) async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        let request = self.build_request(model_id, messages, system, tools, call_options);

        let request_builder = self.http_client.post(&self.base_url).json(&request);

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                debug!(target: "openai::responses", "Cancellation token triggered before sending request.");
                return Err(ApiError::Cancelled{ provider: "openai".to_string()});
            }
            res = request_builder.send() => {
                res.map_err(|e| {
                    error!(
                        target: "openai::responses",
                        "Request send failed: {}",
                        e
                    );
                    ApiError::Network(e)
                })?
            }
        };

        if token.is_cancelled() {
            debug!(target: "openai::responses", "Cancellation token triggered after sending request, before status check.");
            return Err(ApiError::Cancelled {
                provider: "openai".to_string(),
            });
        }

        let status = response.status();

        let body_text = if !status.is_success() {
            // For error responses, also handle cancellation
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    debug!(target: "openai::responses", "Cancellation token triggered while reading error response body.");
                    return Err(ApiError::Cancelled{ provider: "openai".to_string()});
                }
                text_res = response.text() => {
                    text_res.map_err(|e| {
                        error!(
                            target: "openai::responses",
                            "Failed to read response body: {}",
                            e
                        );
                        ApiError::ResponseParsingError {
                            provider: "openai".to_string(),
                            details: e.to_string(),
                        }
                    })?
                }
            }
        } else {
            // For successful responses, also handle cancellation
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    debug!(target: "openai::responses", "Cancellation token triggered while reading successful response body.");
                    return Err(ApiError::Cancelled { provider: "openai".to_string() });
                }
                text_res = response.text() => {
                    text_res.map_err(|e| {
                        error!(
                            target: "openai::responses",
                            "Failed to read response body: {}",
                            e
                        );
                        ApiError::ResponseParsingError {
                            provider: "openai".to_string(),
                            details: e.to_string(),
                        }
                    })?
                }
            }
        };

        // If the request failed, try to parse as error
        if !status.is_success() {
            error!(
                target: "openai::responses",
                "Request failed with status {}: {}",
                status,
                &body_text
            );

            // Try to parse as an OpenAI error response
            if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(&body_text) {
                if let Some(error) = err_json.get("error") {
                    let message = error
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Unknown error");
                    return Err(ApiError::ServerError {
                        provider: "openai".to_string(),
                        status_code: status.as_u16(),
                        details: message.to_string(),
                    });
                }
            }

            return Err(ApiError::ServerError {
                provider: "openai".to_string(),
                status_code: status.as_u16(),
                details: body_text,
            });
        }

        let parsed: ResponsesApiResponse = serde_json::from_str(&body_text).map_err(|e| {
            error!(
                target: "openai::responses",
                "Failed to parse response JSON: {}. Body: {}",
                e,
                &body_text
            );
            ApiError::ResponseParsingError {
                provider: "openai".to_string(),
                details: format!("Invalid response format: {e}"),
            }
        })?;

        Ok(self.convert_response(parsed))
    }

    pub(super) async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        let mut request = self.build_request(model_id, messages, system, tools, call_options);
        request.stream = Some(true);

        let response = tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(ApiError::Cancelled{ provider: "openai".to_string()});
            }
            res = self.http_client.post(&self.base_url).json(&request).send() => {
                res.map_err(ApiError::Network)?
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            error!(
                target: "openai::responses::stream",
                "Request failed with status {}: {}", status, body
            );
            return Err(ApiError::ServerError {
                provider: "openai".to_string(),
                status_code: status.as_u16(),
                details: body,
            });
        }

        let byte_stream = response.bytes_stream();
        let sse_stream = parse_sse_stream(byte_stream);

        Ok(Box::pin(Self::convert_responses_stream(sse_stream, token)))
    }

    fn convert_responses_stream(
        mut sse_stream: impl futures::Stream<Item = Result<crate::api::sse::SseEvent, ApiError>>
        + Unpin
        + Send
        + 'static,
        token: CancellationToken,
    ) -> impl futures::Stream<Item = StreamChunk> + Send + 'static {
        async_stream::stream! {
            let mut text_buffer = String::new();
            let mut thinking_buffer = String::new();
            let mut tool_calls: std::collections::HashMap<String, (String, String)> =
                std::collections::HashMap::new();
            let mut tool_calls_started: std::collections::HashSet<String> =
                std::collections::HashSet::new();

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

                match event.event_type.as_deref() {
                    Some("response.output_text.delta") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                text_buffer.push_str(delta);
                                yield StreamChunk::TextDelta(delta.to_string());
                            }
                        }
                    }
                    Some("response.reasoning.delta") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                thinking_buffer.push_str(delta);
                                yield StreamChunk::ThinkingDelta(delta.to_string());
                            }
                        }
                    }
                    Some("response.function_call_arguments.delta") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            let call_id = extract_non_empty_str(&data, "call_id")
                                .or_else(|| extract_non_empty_str(&data, "id"));
                            let Some(call_id) = call_id else {
                                debug!(
                                    target: "openai::responses::stream",
                                    "Ignoring function_call_arguments.delta without call_id: {}",
                                    event.data
                                );
                                continue;
                            };
                            if let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                let entry = tool_calls.entry(call_id.clone())
                                    .or_insert_with(|| (String::new(), String::new()));
                                entry.1.push_str(delta);
                                yield StreamChunk::ToolUseInputDelta {
                                    id: call_id,
                                    delta: delta.to_string(),
                                };
                            }
                        }
                    }
                    Some("response.function_call.created") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            let call_id = extract_non_empty_str(&data, "call_id")
                                .or_else(|| extract_non_empty_str(&data, "id"));
                            let Some(call_id) = call_id else {
                                debug!(
                                    target: "openai::responses::stream",
                                    "Ignoring function_call.created without call_id: {}",
                                    event.data
                                );
                                continue;
                            };
                            let name = extract_non_empty_str(&data, "name").unwrap_or_default();
                            let entry = tool_calls
                                .entry(call_id.clone())
                                .or_insert_with(|| (String::new(), String::new()));
                            if entry.0.is_empty() && !name.is_empty() {
                                entry.0 = name.clone();
                            }
                            if !name.is_empty() && tool_calls_started.insert(call_id.clone()) {
                                yield StreamChunk::ToolUseStart {
                                    id: call_id,
                                    name,
                                };
                            }
                        }
                    }
                    Some("response.output_item.added") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            if let Some(item) = data.get("item") {
                                let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                if item_type == "function_call" {
                                    let call_id = extract_non_empty_str(item, "call_id")
                                        .or_else(|| extract_non_empty_str(item, "id"))
                                        .or_else(|| extract_non_empty_str(item, "item_id"));
                                    let Some(call_id) = call_id else {
                                        debug!(
                                            target: "openai::responses::stream",
                                            "Ignoring output_item.added without call_id: {}",
                                            event.data
                                        );
                                        continue;
                                    };
                                    let name = extract_non_empty_str(item, "name").unwrap_or_default();
                                    let entry = tool_calls
                                        .entry(call_id.clone())
                                        .or_insert_with(|| (String::new(), String::new()));
                                    if entry.0.is_empty() && !name.is_empty() {
                                        entry.0 = name.clone();
                                    }
                                    if !name.is_empty() && tool_calls_started.insert(call_id.clone()) {
                                        yield StreamChunk::ToolUseStart {
                                            id: call_id,
                                            name,
                                        };
                                    }
                                }
                            }
                        }
                    }
                    Some("response.completed") => {
                        let mut content = Vec::new();

                        if !thinking_buffer.is_empty() {
                            content.push(AssistantContent::Thought {
                                thought: ThoughtContent::Simple {
                                    text: std::mem::take(&mut thinking_buffer),
                                },
                            });
                        }

                        if !text_buffer.is_empty() {
                            content.push(AssistantContent::Text {
                                text: std::mem::take(&mut text_buffer),
                            });
                        }

                        for (id, (name, args)) in std::mem::take(&mut tool_calls) {
                            if id.is_empty() || name.is_empty() {
                                debug!(
                                    target: "openai::responses::stream",
                                    "Skipping tool call with missing id/name: id='{}' name='{}'",
                                    id,
                                    name
                                );
                                continue;
                            }
                            let parameters = serde_json::from_str(&args)
                                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                            content.push(AssistantContent::ToolCall {
                                tool_call: steer_tools::ToolCall {
                                    id,
                                    name,
                                    parameters,
                                },
                            });
                        }

                        yield StreamChunk::MessageComplete(CompletionResponse { content });
                        break;
                    }
                    Some("error") => {
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                            let message = data.get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("Unknown error");
                            yield StreamChunk::Error(StreamError::Provider {
                                provider: "openai".into(),
                                error_type: "stream_error".into(),
                                message: message.to_string(),
                            });
                            break;
                        }
                    }
                    _ => {
                        debug!(
                            target: "openai::responses::stream",
                            "Unhandled event type: {:?}", event.event_type
                        );
                    }
                }
            }
        }
    }
}

fn extract_non_empty_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};

    use steer_tools::ToolSchema;

    #[test]
    fn test_responses_api_message_conversion() {
        let client = Client::new("test_key".to_string());

        let messages = vec![
            Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: "Hello".to_string(),
                    }],
                },
                timestamp: 1000,
                id: "msg1".to_string(),
                parent_message_id: None,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: "Hi there!".to_string(),
                    }],
                },
                timestamp: 2000,
                id: "msg2".to_string(),
                parent_message_id: Some("msg1".to_string()),
            },
        ];

        let actual = client.convert_messages_to_input(&messages);
        let expected = Some(InputType::Messages(vec![
            InputItem::Message {
                role: "user".to_string(),
                content: vec![InputContentPart::InputText {
                    text: "Hello".to_string(),
                }],
            },
            InputItem::Message {
                role: "assistant".to_string(),
                content: vec![InputContentPart::OutputText {
                    text: "Hi there!".to_string(),
                    annotations: vec![],
                }],
            },
        ]));

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_tool_conversion() {
        let client = Client::new("test_key".to_string());

        let tools = vec![ToolSchema {
            name: "get_weather".to_string(),
            description: "Get the weather".to_string(),
            input_schema: steer_tools::InputSchema {
                schema_type: "object".to_string(),
                properties: serde_json::json!({
                    "location": {
                        "type": "string",
                        "description": "City name"
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
                required: vec!["location".to_string()],
            },
        }];

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What's the weather?".to_string(),
                }],
            },
            timestamp: 1000,
            id: "msg1".to_string(),
            parent_message_id: None,
        }];

        let model_id = (
            crate::config::provider::openai(),
            "gpt-4.1-2025-04-14".to_string(),
        );
        let request = client.build_request(
            &model_id,
            messages,
            Some("You are a weather assistant".to_string()),
            Some(tools),
            None, // No call options for this test
        );

        assert!(request.tools.is_some());
        let tools = request.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "get_weather");
        assert!(!tools[0].strict);

        assert!(request.tool_choice.is_some());
    }

    #[test]
    fn test_responses_api_reasoning_config() {
        let client = Client::new("test_key".to_string());

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Solve a complex problem".to_string(),
                }],
            },
            timestamp: 1000,
            id: "msg1".to_string(),
            parent_message_id: None,
        }];

        // Test with reasoning model (with call options enabling thinking)
        let model_id = (
            crate::config::provider::openai(),
            "codex-mini-latest".to_string(),
        );
        let call_options = Some(crate::config::model::ModelParameters {
            temperature: None,
            max_tokens: None,
            top_p: None,
            thinking_config: Some(crate::config::model::ThinkingConfig {
                enabled: true,
                ..Default::default()
            }),
        });
        let request = client.build_request(&model_id, messages.clone(), None, None, call_options);

        assert!(request.reasoning.is_some());
        let reasoning = request.reasoning.unwrap();
        assert_eq!(
            reasoning.effort,
            Some(crate::api::openai::responses_types::ReasoningEffort::Medium)
        );

        // Test with non-reasoning model (no thinking config)
        let model_id = (
            crate::config::provider::openai(),
            "gpt-4.1-2025-04-14".to_string(),
        );
        let request = client.build_request(&model_id, messages, None, None, None);

        assert!(request.reasoning.is_none());
    }

    #[test]
    fn test_responses_api_tool_result_conversion() {
        let client = Client::new("test_key".to_string());

        let messages = vec![
            Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: "List files".to_string(),
                    }],
                },
                timestamp: 1000,
                id: "msg1".to_string(),
                parent_message_id: None,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: steer_tools::ToolCall {
                            id: "call_123".to_string(),
                            name: "ls".to_string(),
                            parameters: serde_json::json!({"path": "."}),
                        },
                    }],
                },
                timestamp: 2000,
                id: "msg2".to_string(),
                parent_message_id: Some("msg1".to_string()),
            },
            Message {
                data: MessageData::Tool {
                    tool_use_id: "call_123".to_string(),
                    result: steer_tools::result::ToolResult::External(
                        steer_tools::result::ExternalResult {
                            tool_name: "ls".to_string(),
                            payload: "file1.txt file2.txt".to_string(),
                        },
                    ),
                },
                timestamp: 3000,
                id: "msg3".to_string(),
                parent_message_id: Some("msg2".to_string()),
            },
        ];

        let actual = client.convert_messages_to_input(&messages);
        let expected = Some(InputType::Messages(vec![
            InputItem::Message {
                role: "user".to_string(),
                content: vec![InputContentPart::InputText {
                    text: "List files".to_string(),
                }],
            },
            InputItem::FunctionCall {
                item_type: "function_call".to_string(),
                call_id: "call_123".to_string(),
                name: "ls".to_string(),
                arguments: "{\"path\":\".\"}".to_string(),
            },
            InputItem::FunctionCallOutput {
                item_type: "function_call_output".to_string(),
                call_id: "call_123".to_string(),
                output: "file1.txt file2.txt".to_string(),
            },
        ]));

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_output_parsing() {
        let client = Client::new("test_key".to_string());

        // Test parsing function call output
        let output = vec![ResponseOutputItem::FunctionCall {
            id: "fc_123".to_string(),
            call_id: "call_456".to_string(),
            name: "get_weather".to_string(),
            arguments: r#"{"location":"Boston"}"#.to_string(),
            status: "completed".to_string(),
        }];

        let actual = client.convert_output(Some(output));
        let expected = vec![AssistantContent::ToolCall {
            tool_call: steer_tools::ToolCall {
                id: "call_456".to_string(),
                name: "get_weather".to_string(),
                parameters: serde_json::json!({"location": "Boston"}),
            },
        }];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_reasoning_extraction() {
        let client = Client::new("test_key".to_string());

        let response = ResponsesApiResponse {
            id: "resp_123".to_string(),
            object: "response".to_string(),
            created_at: 1234567890,
            status: "completed".to_string(),
            error: None,
            output: Some(vec![
                ResponseOutputItem::Reasoning {
                    id: "rs_123".to_string(),
                    summary: vec![ReasoningSummaryPart::SummaryText {
                        text: "Let me think step by step...".to_string(),
                    }],
                    content: None,
                    encrypted_content: None,
                    status: Some("completed".to_string()),
                },
                ResponseOutputItem::Message {
                    id: "msg_123".to_string(),
                    status: "completed".to_string(),
                    role: "assistant".to_string(),
                    content: vec![MessageContentPart::OutputText {
                        text: "The answer is 42.".to_string(),
                        annotations: vec![],
                    }],
                },
            ]),
            model: Some("o3-mini".to_string()),
            reasoning: None,
            usage: None,
            extra: Default::default(),
        };

        let actual = client.convert_response(response);
        let expected = CompletionResponse {
            content: vec![
                AssistantContent::Thought {
                    thought: crate::app::conversation::ThoughtContent::Simple {
                        text: "Let me think step by step...".to_string(),
                    },
                },
                AssistantContent::Text {
                    text: "The answer is 42.".to_string(),
                },
            ],
        };

        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn test_convert_responses_stream_text_deltas() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.output_text.delta".to_string()),
                data: r#"{"delta":"Hello"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.output_text.delta".to_string()),
                data: r#"{"delta":" world"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r#"{}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let first_delta = stream.next().await.unwrap();
        assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "Hello"));

        let second_delta = stream.next().await.unwrap();
        assert!(matches!(second_delta, StreamChunk::TextDelta(ref t) if t == " world"));

        let complete = stream.next().await.unwrap();
        assert!(matches!(complete, StreamChunk::MessageComplete(_)));
    }

    #[tokio::test]
    async fn test_convert_responses_stream_with_reasoning() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.reasoning.delta".to_string()),
                data: r#"{"delta":"Thinking..."}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.output_text.delta".to_string()),
                data: r#"{"delta":"Result"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r#"{}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let thinking_delta = stream.next().await.unwrap();
        assert!(matches!(thinking_delta, StreamChunk::ThinkingDelta(ref t) if t == "Thinking..."));

        let text_delta = stream.next().await.unwrap();
        assert!(matches!(text_delta, StreamChunk::TextDelta(ref t) if t == "Result"));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 2);
            assert!(matches!(
                &response.content[0],
                AssistantContent::Thought { .. }
            ));
            assert!(
                matches!(&response.content[1], AssistantContent::Text { text } if text == "Result")
            );
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_responses_stream_with_function_call() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.function_call.created".to_string()),
                data: r#"{"call_id":"call_123","name":"get_weather"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"{\"city\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"\"NYC\"}"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r#"{}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let tool_start = stream.next().await.unwrap();
        assert!(
            matches!(tool_start, StreamChunk::ToolUseStart { ref id, ref name } if id == "call_123" && name == "get_weather")
        );

        let arg_delta_1 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_1, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "{\"city\":")
        );

        let arg_delta_2 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_2, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "\"NYC\"}")
        );

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            if let AssistantContent::ToolCall { tool_call } = &response.content[0] {
                assert_eq!(tool_call.id, "call_123");
                assert_eq!(tool_call.name, "get_weather");
            } else {
                panic!("Expected ToolCall");
            }
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_responses_stream_with_output_item_call_id() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.output_item.added".to_string()),
                data: r#"{"item":{"type":"function_call","id":"item_1","call_id":"call_123","name":"get_weather"}}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"{\"city\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"\"NYC\"}"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r#"{}"#.to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let tool_start = stream.next().await.unwrap();
        assert!(
            matches!(tool_start, StreamChunk::ToolUseStart { ref id, ref name } if id == "call_123" && name == "get_weather")
        );

        let arg_delta_1 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_1, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "{\"city\":")
        );

        let arg_delta_2 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_2, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "\"NYC\"}")
        );

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            if let AssistantContent::ToolCall { tool_call } = &response.content[0] {
                assert_eq!(tool_call.id, "call_123");
                assert_eq!(tool_call.name, "get_weather");
            } else {
                panic!("Expected ToolCall");
            }
        } else {
            panic!("Expected MessageComplete");
        }
    }

    #[tokio::test]
    async fn test_convert_responses_stream_cancellation() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("response.output_text.delta".to_string()),
            data: r#"{"delta":"Hello"}"#.to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        token.cancel();

        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let cancelled = stream.next().await.unwrap();
        assert!(matches!(
            cancelled,
            StreamChunk::Error(StreamError::Cancelled)
        ));
    }
}
