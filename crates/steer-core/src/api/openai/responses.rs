use futures::StreamExt;
use reqwest::{self, header};
use serde_json;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use crate::api::error::{ApiError, SseParseError, StreamError};
use crate::api::openai::responses_types::{
    ExtraValue, InputContentPart, InputItem, InputType, MessageContentPart, ReasoningConfig,
    ReasoningSummary, ReasoningSummaryPart, ResponseError, ResponseErrorEvent, ResponseFailedEvent,
    ResponseOutputItem, ResponsesApiResponse, ResponsesFunctionTool, ResponsesHttpErrorEnvelope,
    ResponsesRequest, ResponsesToolChoice,
};
use crate::api::provider::{CompletionResponse, CompletionStream, StreamChunk};
use crate::api::sse::parse_sse_stream;
use crate::app::SystemContext;
use crate::app::conversation::{
    AssistantContent, Message as AppMessage, MessageData, ThoughtContent, UserContent,
};
use crate::auth::{
    AuthErrorAction, AuthErrorContext, AuthHeaderContext, InstructionPolicy, OpenAiResponsesAuth,
    RequestKind,
};
use crate::auth::{ModelId as AuthModelId, ProviderId as AuthProviderId};
use crate::config::model::{ModelId, ModelParameters};
use steer_tools::ToolSchema;

const DEFAULT_API_URL: &str = "https://api.openai.com/v1/responses";
#[derive(Clone)]
enum ResponsesAuth {
    ApiKey(String),
    Directive(OpenAiResponsesAuth),
}

impl ResponsesAuth {
    fn directive(&self) -> Option<&OpenAiResponsesAuth> {
        match self {
            ResponsesAuth::Directive(directive) => Some(directive),
            ResponsesAuth::ApiKey(_) => None,
        }
    }
}

#[derive(Clone)]
pub(super) struct Client {
    http: reqwest::Client,
    base_url: String,
    auth: ResponsesAuth,
}

impl Client {
    pub(super) fn new(api_key: String) -> Result<Self, ApiError> {
        Self::with_base_url(api_key, None)
    }

    pub(super) fn with_base_url(
        api_key: String,
        base_url: Option<String>,
    ) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(super::HTTP_TIMEOUT_SECS))
            .build()
            .map_err(ApiError::Network)?;

        let base_url =
            crate::api::util::normalize_responses_url(base_url.as_deref(), DEFAULT_API_URL);

        Ok(Self {
            http,
            base_url,
            auth: ResponsesAuth::ApiKey(api_key),
        })
    }

    pub(super) fn with_directive(
        directive: OpenAiResponsesAuth,
        base_url: Option<String>,
    ) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(super::HTTP_TIMEOUT_SECS))
            .build()
            .map_err(ApiError::Network)?;

        let base_url = directive
            .base_url_override
            .as_deref()
            .or(base_url.as_deref());
        let base_url = crate::api::util::normalize_responses_url(base_url, DEFAULT_API_URL);

        Ok(Self {
            http,
            base_url,
            auth: ResponsesAuth::Directive(directive),
        })
    }

    /// Build a request with proper message structure and tool support
    pub(crate) fn build_request(
        &self,
        model_id: &ModelId,
        messages: Vec<AppMessage>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
    ) -> ResponsesRequest {
        let directive = self.auth.directive();
        let instructions = apply_instruction_policy(
            system,
            directive.and_then(|d| d.instruction_policy.as_ref()),
        );

        let input = Self::convert_messages_to_input(&messages);

        let responses_tools = tools.map(|tools| {
            tools
                .into_iter()
                .map(|tool| ResponsesFunctionTool {
                    parameters: tool.input_schema.as_value().clone(),
                    tool_type: "function".to_string(),
                    name: tool.name,
                    description: Some(tool.description),
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
                    crate::config::toml_types::ThinkingEffort::XHigh => {
                        Some(crate::api::openai::responses_types::ReasoningEffort::XHigh)
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

        let mut request = ResponsesRequest {
            model: model_id.id.clone(), // Use the model ID string
            input,
            instructions,
            previous_response_id: None,
            temperature: call_options.as_ref().and_then(|o| o.temperature),
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
        };

        if let Some(include) = directive.and_then(|d| d.include.as_ref()) {
            let values = include.iter().cloned().map(ExtraValue::String).collect();
            request
                .extra
                .insert("include".to_string(), ExtraValue::Array(values));
        }

        request
    }

    async fn auth_headers(&self, ctx: AuthHeaderContext) -> Result<header::HeaderMap, ApiError> {
        match &self.auth {
            ResponsesAuth::ApiKey(value) => {
                let mut headers = header::HeaderMap::new();
                headers.insert(
                    header::AUTHORIZATION,
                    header::HeaderValue::from_str(&format!("Bearer {value}")).map_err(|e| {
                        ApiError::AuthenticationFailed {
                            provider: "openai".to_string(),
                            details: format!("Invalid API key: {e}"),
                        }
                    })?,
                );
                Ok(headers)
            }
            ResponsesAuth::Directive(directive) => {
                let header_pairs = directive
                    .headers
                    .headers(ctx)
                    .await
                    .map_err(|e| ApiError::AuthError(e.to_string()))?;
                let mut headers = header::HeaderMap::new();
                for pair in header_pairs {
                    let name =
                        header::HeaderName::from_bytes(pair.name.as_bytes()).map_err(|e| {
                            ApiError::AuthenticationFailed {
                                provider: "openai".to_string(),
                                details: format!("Invalid header name: {e}"),
                            }
                        })?;
                    let value = header::HeaderValue::from_str(&pair.value).map_err(|e| {
                        ApiError::AuthenticationFailed {
                            provider: "openai".to_string(),
                            details: format!("Invalid header value: {e}"),
                        }
                    })?;
                    headers.insert(name, value);
                }
                Ok(headers)
            }
        }
    }

    async fn on_auth_error(
        &self,
        status: u16,
        body: &str,
        request_kind: RequestKind,
    ) -> Result<AuthErrorAction, ApiError> {
        let Some(directive) = self.auth.directive() else {
            return Ok(AuthErrorAction::NoAction);
        };
        let context = AuthErrorContext {
            status: Some(status),
            body_snippet: Some(truncate_body(body)),
            request_kind,
        };
        directive
            .headers
            .on_auth_error(context)
            .await
            .map_err(|e| ApiError::AuthError(e.to_string()))
    }

    pub(super) fn convert_output(output: Option<Vec<ResponseOutputItem>>) -> Vec<AssistantContent> {
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
                                MessageContentPart::Refusal { refusal } => {
                                    result.push(AssistantContent::Text { text: refusal });
                                }
                                MessageContentPart::Other => {}
                            }
                        }
                    }
                    ResponseOutputItem::Reasoning { summary, .. } => {
                        // Extract reasoning text from summary parts
                        let mut reasoning_text = String::new();
                        for part in summary {
                            if let Some(text) = part.text() {
                                if !reasoning_text.is_empty() {
                                    reasoning_text.push('\n');
                                }
                                reasoning_text.push_str(text);
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
                        id,
                        call_id,
                        name,
                        arguments,
                        ..
                    } => {
                        let Some(call_id) = select_call_id(call_id, Some(id)) else {
                            continue;
                        };
                        let parameters = parse_tool_parameters(&name, &arguments);

                        result.push(AssistantContent::ToolCall {
                            tool_call: steer_tools::ToolCall {
                                id: call_id,
                                name,
                                parameters,
                            },
                            thought_signature: None,
                        });
                    }
                    ResponseOutputItem::CustomToolCall {
                        id,
                        call_id,
                        name,
                        tool_name,
                        input,
                        ..
                    } => {
                        let Some(call_id) = select_call_id(call_id, id) else {
                            continue;
                        };
                        let Some(name) = select_tool_name(name, tool_name) else {
                            continue;
                        };
                        let input = input.unwrap_or_default();
                        let parameters = parse_tool_parameters(&name, &input);
                        result.push(AssistantContent::ToolCall {
                            tool_call: steer_tools::ToolCall {
                                id: call_id,
                                name,
                                parameters,
                            },
                            thought_signature: None,
                        });
                    }
                    ResponseOutputItem::McpCall {
                        id,
                        call_id,
                        name,
                        tool_name,
                        arguments,
                        ..
                    } => {
                        let Some(call_id) = select_call_id(call_id, id) else {
                            continue;
                        };
                        let Some(name) = select_tool_name(name, tool_name) else {
                            continue;
                        };
                        let arguments = arguments.unwrap_or_default();
                        let parameters = parse_tool_parameters(&name, &arguments);
                        result.push(AssistantContent::ToolCall {
                            tool_call: steer_tools::ToolCall {
                                id: call_id,
                                name,
                                parameters,
                            },
                            thought_signature: None,
                        });
                    }
                    ResponseOutputItem::WebSearchCall { .. }
                    | ResponseOutputItem::FileSearchCall { .. }
                    | ResponseOutputItem::McpApprovalRequest { .. }
                    | ResponseOutputItem::CodeInterpreterCall { .. }
                    | ResponseOutputItem::ImageGenerationCall { .. }
                    | ResponseOutputItem::Other => {
                        // These are built-in tools that we don't handle yet
                    }
                }
            }
        }
        result
    }

    /// Convert messages to the structured input format that preserves roles
    pub(super) fn convert_messages_to_input(messages: &[AppMessage]) -> Option<InputType> {
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
                            UserContent::CommandExecution {
                                command,
                                stdout,
                                stderr,
                                exit_code,
                            } => {
                                // Format command execution as XML-formatted text
                                let formatted = UserContent::format_command_execution_as_xml(
                                    command, stdout, stderr, *exit_code,
                                );
                                content_parts.push(InputContentPart::InputText { text: formatted });
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
                            AssistantContent::ToolCall { tool_call, .. } => {
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
    pub(super) fn convert_response(response: ResponsesApiResponse) -> CompletionResponse {
        let content = Self::convert_output(response.output);

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
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        if let Some(directive) = self.auth.directive()
            && directive.require_streaming.unwrap_or(false)
        {
            return Err(ApiError::Configuration(
                "OpenAI OAuth requests require streaming responses".to_string(),
            ));
        }

        let auth_ctx = auth_header_context(model_id, RequestKind::Complete);
        let mut attempts = 0;

        loop {
            let request = self.build_request(
                model_id,
                messages.clone(),
                system.clone(),
                tools.clone(),
                call_options,
            );
            log_request_payload(&request, false);
            let headers = self.auth_headers(auth_ctx.clone()).await?;
            let request_builder = self
                .http
                .post(&self.base_url)
                .headers(headers)
                .json(&request);

            let response = tokio::select! {
                biased;
                () = token.cancelled() => {
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

            let body_text = if status.is_success() {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
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
            } else {
                tokio::select! {
                    biased;
                    () = token.cancelled() => {
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
            };

            if !status.is_success() {
                if is_auth_status(status) && self.auth.directive().is_some() {
                    let action = self
                        .on_auth_error(status.as_u16(), &body_text, RequestKind::Complete)
                        .await?;
                    if matches!(action, AuthErrorAction::RetryOnce) && attempts == 0 {
                        attempts += 1;
                        continue;
                    }
                    return Err(ApiError::AuthenticationFailed {
                        provider: "openai".to_string(),
                        details: body_text,
                    });
                }

                error!(
                    target: "openai::responses",
                    "Request failed with status {}: {}",
                    status,
                    &body_text
                );

                return Err(parse_http_error_response(status.as_u16(), body_text));
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

            return Ok(Self::convert_response(parsed));
        }
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
        let auth_ctx = auth_header_context(model_id, RequestKind::Stream);
        let mut attempts = 0;

        loop {
            let mut request = self.build_request(
                model_id,
                messages.clone(),
                system.clone(),
                tools.clone(),
                call_options,
            );
            request.stream = Some(true);
            log_request_payload(&request, true);

            let headers = self.auth_headers(auth_ctx.clone()).await?;
            let request_builder = self
                .http
                .post(&self.base_url)
                .headers(headers)
                .json(&request);

            let response = tokio::select! {
                biased;
                () = token.cancelled() => {
                    debug!(target: "openai::responses::stream", "Cancellation token triggered before sending request.");
                    return Err(ApiError::Cancelled{ provider: "openai".to_string()});
                }
                res = request_builder.send() => {
                    res.map_err(|e| {
                        error!(target: "openai::responses::stream", "Request send failed: {}", e);
                        ApiError::Network(e)
                    })?
                }
            };

            if token.is_cancelled() {
                debug!(target: "openai::responses::stream", "Cancellation token triggered after sending request.");
                return Err(ApiError::Cancelled {
                    provider: "openai".to_string(),
                });
            }

            let status = response.status();

            if !status.is_success() {
                let body_text = tokio::select! {
                    biased;
                    () = token.cancelled() => {
                        debug!(target: "openai::responses::stream", "Cancellation token triggered while reading error response body.");
                        return Err(ApiError::Cancelled{ provider: "openai".to_string()});
                    }
                    text_res = response.text() => {
                        text_res.map_err(|e| {
                            error!(
                                target: "openai::responses::stream",
                                "Failed to read response body: {}",
                                e
                            );
                            ApiError::ResponseParsingError {
                                provider: "openai".to_string(),
                                details: e.to_string(),
                            }
                        })?
                    }
                };

                if is_auth_status(status) && self.auth.directive().is_some() {
                    let action = self
                        .on_auth_error(status.as_u16(), &body_text, RequestKind::Stream)
                        .await?;
                    if matches!(action, AuthErrorAction::RetryOnce) && attempts == 0 {
                        attempts += 1;
                        continue;
                    }
                    return Err(ApiError::AuthenticationFailed {
                        provider: "openai".to_string(),
                        details: body_text,
                    });
                }

                error!(
                    target: "openai::responses::stream",
                    "Request failed with status {}: {}",
                    status,
                    &body_text
                );

                return Err(parse_http_error_response(status.as_u16(), body_text));
            }

            let byte_stream = response.bytes_stream();
            let sse_stream = parse_sse_stream(byte_stream);

            return Ok(Box::pin(Client::convert_responses_stream(
                sse_stream, token,
            )));
        }
    }

    pub(crate) fn convert_responses_stream(
        mut sse_stream: impl futures::Stream<Item = Result<crate::api::sse::SseEvent, SseParseError>>
        + Unpin
        + Send
        + 'static,
        token: CancellationToken,
    ) -> impl futures::Stream<Item = StreamChunk> + Send + 'static {
        async_stream::stream! {
                    let mut content: Vec<AssistantContent> = Vec::new();
                    let mut tool_call_keys: Vec<Option<String>> = Vec::new();
                    let mut tool_calls: std::collections::HashMap<String, (String, String)> =
                        std::collections::HashMap::new();
                    let mut tool_calls_started: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut item_to_call_id: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    let mut tool_call_positions: std::collections::HashMap<String, usize> =
                        std::collections::HashMap::new();
                    let mut pending_tool_deltas: std::collections::HashMap<String, Vec<String>> =
                        std::collections::HashMap::new();
                    let mut summary_deltas_seen: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut summary_done_emitted: std::collections::HashSet<String> =
                        std::collections::HashSet::new();

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

                        match event.event_type.as_deref() {
                            Some("response.output_text.delta" | "response.refusal.delta") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data)
                                    && let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                        append_text_delta(&mut content, &mut tool_call_keys, delta);
                                        yield StreamChunk::TextDelta(delta.to_string());
                                    }
                            }
                            Some("response.reasoning.delta" | "response.reasoning_text.delta" |
        "response.reasoning_summary_text.delta") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data)
                                    && let Some(delta) = data.get("delta").and_then(|d| d.as_str()) {
                                        if matches!(event.event_type.as_deref(), Some("response.reasoning_summary_text.delta"))
                                            && let Some(key) = summary_key_from_event(&data) {
                                                summary_deltas_seen.insert(key);
                                            }
                                        append_thinking_delta(&mut content, &mut tool_call_keys, delta);
                                        yield StreamChunk::ThinkingDelta(delta.to_string());
                                    }
                            }
                            Some("response.reasoning_summary_text.done" |
        "response.reasoning_summary_part.done") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data)
                                    && let Some(text) = summary_text_from_event(&data) {
                                        let key = summary_key_from_event(&data);
                                        if should_emit_summary(&key, &summary_deltas_seen, &summary_done_emitted) {
                                            append_thinking_delta(&mut content, &mut tool_call_keys, &text);
                                            yield StreamChunk::ThinkingDelta(text.clone());
                                            if let Some(key) = key {
                                                summary_done_emitted.insert(key);
                                            }
                                        }
                                    }
                            }
                            Some("response.in_progress" | "response.reasoning_summary_part.added" |
        "response.created" | "response.output_item.done" | "response.output_text.done"
        | "response.content_part.added" | "response.content_part.done" |
        "response.refusal.done") => {
                                // No-op: informational events that don't affect streamed content.
                            }
                            Some("response.function_call_arguments.delta" |
        "response.custom_tool_call_input.delta" | "response.mcp_call_arguments.delta") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                    let mut tool_state = ToolCallState::new(
                                        &mut tool_calls,
                                        &mut tool_call_positions,
                                        &mut content,
                                        &mut tool_call_keys,
                                        &mut pending_tool_deltas,
                                        &mut tool_calls_started,
                                    );
                                    match handle_tool_call_delta(
                                        &data,
                                        &item_to_call_id,
                                        &mut tool_state,
                                        &["delta", "input", "arguments"],
                                    ) {
                                        ToolDeltaOutcome::Emit { id, delta } => {
                                            yield StreamChunk::ToolUseInputDelta { id, delta };
                                        }
                                        ToolDeltaOutcome::Buffered => {}
                                        ToolDeltaOutcome::Missing => {
                                            debug!(
                                                target: "openai::responses::stream",
                                                "Ignoring tool_call delta without call_id or delta: {}",
                                                event.data
                                            );
                                        }
                                    }
                                }
                            }
                            Some("response.function_call_arguments.done" |
        "response.custom_tool_call_input.done" | "response.mcp_call_arguments.done") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                    let mut tool_state = ToolCallState::new(
                                        &mut tool_calls,
                                        &mut tool_call_positions,
                                        &mut content,
                                        &mut tool_call_keys,
                                        &mut pending_tool_deltas,
                                        &mut tool_calls_started,
                                    );
                                    handle_tool_call_done(
                                        &data,
                                        &item_to_call_id,
                                        &mut tool_state,
                                        &["arguments", "input", "delta"],
                                    );
                                }
                            }
                            Some("response.function_call.created") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data) {
                                    let call_id = resolve_call_id(&data, &item_to_call_id);
                                    let Some(call_id) = call_id else {
                                        debug!(
                                            target: "openai::responses::stream",
                                            "Ignoring function_call.created without call_id: {}",
                                            event.data
                                        );
                                        continue;
                                    };
                                    let name = extract_first_non_empty_str(&data, &["name", "tool_name"]);
                                    let args = extract_first_non_empty_str(&data, &["arguments", "input"]);
                                    let mut tool_state = ToolCallState::new(
                                        &mut tool_calls,
                                        &mut tool_call_positions,
                                        &mut content,
                                        &mut tool_call_keys,
                                        &mut pending_tool_deltas,
                                        &mut tool_calls_started,
                                    );
                                    if let Some(chunk) = register_tool_call(&call_id, name, args, &mut tool_state) {
                                        yield chunk;
                                    }
                                    if tool_calls_started.contains(&call_id) {
                                        for delta in drain_pending_tool_deltas(
                                            &call_id,
                                            &mut pending_tool_deltas,
                                        ) {
                                            yield StreamChunk::ToolUseInputDelta {
                                                id: call_id.clone(),
                                                delta,
                                            };
                                        }
                                    }
                                }
                            }
                            Some("response.output_item.added") => {
                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&event.data)
                                    && let Some(item) = data.get("item") {
                                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                        if matches!(item_type, "function_call" | "custom_tool_call" | "mcp_call") {
                                            let item_id = extract_non_empty_str(item, "id")
                                                .or_else(|| extract_non_empty_str(item, "item_id"));
                                            let call_id = resolve_call_id(item, &item_to_call_id);
                                            let Some(call_id) = call_id else {
                                                debug!(
                                                    target: "openai::responses::stream",
                                                    "Ignoring output_item.added without call_id: {}",
                                                    event.data
                                                );
                                                continue;
                                            };
                                            if let Some(item_id) = item_id
                                                && !item_id.is_empty() {
                                                    item_to_call_id.insert(item_id.clone(), call_id.clone());
                                                    let mut tool_state = ToolCallState::new(
                                                        &mut tool_calls,
                                                        &mut tool_call_positions,
                                                        &mut content,
                                                        &mut tool_call_keys,
                                                        &mut pending_tool_deltas,
                                                        &mut tool_calls_started,
                                                    );
                                                    promote_tool_call_id(&item_id, &call_id, &mut tool_state);
                                                }
                                            let name = extract_first_non_empty_str(item, &["name", "tool_name"]);
                                            let args = extract_first_non_empty_str(item, &["arguments", "input"]);
                                            let mut tool_state = ToolCallState::new(
                                                &mut tool_calls,
                                                &mut tool_call_positions,
                                                &mut content,
                                                &mut tool_call_keys,
                                                &mut pending_tool_deltas,
                                                &mut tool_calls_started,
                                            );
                                            if let Some(chunk) = register_tool_call(&call_id, name, args, &mut tool_state)
                                            {
                                                yield chunk;
                                            }
                                            if tool_calls_started.contains(&call_id) {
                                                for delta in drain_pending_tool_deltas(
                                                    &call_id,
                                                    &mut pending_tool_deltas,
                                                ) {
                                                    yield StreamChunk::ToolUseInputDelta {
                                                        id: call_id.clone(),
                                                        delta,
                                                    };
                                                }
                                            }
                                        }
                                    }
                            }
                            Some("response.completed") => {
                                let tool_calls = std::mem::take(&mut tool_calls);
                                let mut final_content = Vec::new();

                                for (block, tool_key) in content.into_iter().zip(tool_call_keys.into_iter())
                                {
                                    if let Some(call_id) = tool_key {
                                        let Some((name, args)) = tool_calls.get(&call_id) else {
                                            continue;
                                        };
                                        if call_id.is_empty() || name.is_empty() {
                                            debug!(
                                                target: "openai::responses::stream",
                                                "Skipping tool call with missing id/name: id='{}' name='{}'",
                                                call_id,
                                                name
                                            );
                                            continue;
                                        }
                                        let parameters = parse_tool_parameters(name, args);
                                        final_content.push(AssistantContent::ToolCall {
                                            tool_call: steer_tools::ToolCall {
                                                id: call_id.clone(),
                                                name: name.clone(),
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
                            Some("error") => {
                                let parsed_event =
                                    serde_json::from_str::<ResponseErrorEvent>(&event.data);
                                match parsed_event {
                                    Ok(error_event) => {
                                        let error_type = error_event
                                            .event_type
                                            .clone()
                                            .unwrap_or_else(|| "stream_error".to_string());
                                        yield StreamChunk::Error(StreamError::Provider {
                                            provider: "openai".into(),
                                            error_type,
                                            message: error_event.message,
                                        });
                                        break;
                                    }
                                    Err(parse_error) => {
                                        debug!(
                                            target: "openai::responses::stream",
                                            "Failed to parse error event payload: {} payload={}",
                                            parse_error,
                                            event.data,
                                        );
                                        yield StreamChunk::Error(StreamError::Provider {
                                            provider: "openai".into(),
                                            error_type: "stream_error".into(),
                                            message: event.data.clone(),
                                        });
                                        break;
                                    }
                                }
                            }
                            Some("response.failed") => {
                                let parsed_event =
                                    serde_json::from_str::<ResponseFailedEvent>(&event.data);
                                match parsed_event {
                                    Ok(failed_event) => {
                                        let response_error = failed_event.response.error;
                                        let fallback = response_error.as_ref().map_or_else(
                                            || "Response failed without an error object".to_string(),
                                            stream_error_message,
                                        );
                                        let (error_type, message) = response_error
                                            .map_or_else(
                                                || ("response_failed".to_string(), fallback),
                                                |error| stream_error_details(&error),
                                            );
                                        yield StreamChunk::Error(StreamError::Provider {
                                            provider: "openai".into(),
                                            error_type,
                                            message,
                                        });
                                        break;
                                    }
                                    Err(parse_error) => {
                                        debug!(
                                            target: "openai::responses::stream",
                                            "Failed to parse response.failed payload: {} payload={}",
                                            parse_error,
                                            event.data,
                                        );
                                        yield StreamChunk::Error(StreamError::Provider {
                                            provider: "openai".into(),
                                            error_type: "response_failed".into(),
                                            message: event.data.clone(),
                                        });
                                        break;
                                    }
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

fn extract_first_non_empty_str(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = extract_non_empty_str(value, key) {
            return Some(value);
        }
    }
    None
}

enum ToolCallIdResolution {
    Resolved(String),
    Pending(String),
}

enum ToolDeltaOutcome {
    Emit { id: String, delta: String },
    Buffered,
    Missing,
}

fn resolve_tool_call_id_for_event(
    value: &serde_json::Value,
    item_to_call_id: &std::collections::HashMap<String, String>,
) -> Option<ToolCallIdResolution> {
    if let Some(call_id) = extract_non_empty_str(value, "call_id") {
        return Some(ToolCallIdResolution::Resolved(call_id));
    }
    if let Some(item_id) = extract_non_empty_str(value, "item_id") {
        if let Some(mapped) = item_to_call_id.get(&item_id) {
            return Some(ToolCallIdResolution::Resolved(mapped.clone()));
        }
        return Some(ToolCallIdResolution::Pending(item_id));
    }
    if let Some(id) = extract_non_empty_str(value, "id") {
        if let Some(mapped) = item_to_call_id.get(&id) {
            return Some(ToolCallIdResolution::Resolved(mapped.clone()));
        }
        return Some(ToolCallIdResolution::Resolved(id));
    }
    None
}

fn summary_index_from_container(value: &serde_json::Value) -> Option<i64> {
    let summary_index = value.get("summary_index")?;
    summary_index.as_i64().or_else(|| {
        summary_index
            .as_str()
            .and_then(|value| value.parse::<i64>().ok())
    })
}

fn summary_index_from_event(data: &serde_json::Value) -> Option<i64> {
    summary_index_from_container(data)
        .or_else(|| data.get("part").and_then(summary_index_from_container))
}

fn summary_key_from_event(data: &serde_json::Value) -> Option<String> {
    let item_key = data
        .get("item_id")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string())
        .or_else(|| {
            data.get("output_index")
                .and_then(|value| value.as_i64())
                .map(|value| format!("output_{value}"))
        })?;
    let summary_index = summary_index_from_event(data)?;
    Some(format!("{item_key}:{summary_index}"))
}

fn summary_text_from_event(data: &serde_json::Value) -> Option<String> {
    if let Some(text) = data.get("text").and_then(|value| value.as_str()) {
        return Some(text.to_string());
    }
    let part = data.get("part")?;
    let part = serde_json::from_value::<ReasoningSummaryPart>(part.clone()).ok()?;
    part.text().map(|text| text.to_string())
}

fn should_emit_summary(
    key: &Option<String>,
    summary_deltas_seen: &std::collections::HashSet<String>,
    summary_done_emitted: &std::collections::HashSet<String>,
) -> bool {
    match key {
        Some(key) => !summary_deltas_seen.contains(key) && !summary_done_emitted.contains(key),
        None => true,
    }
}

struct ToolCallState<'a> {
    tool_calls: &'a mut std::collections::HashMap<String, (String, String)>,
    tool_call_positions: &'a mut std::collections::HashMap<String, usize>,
    content: &'a mut Vec<AssistantContent>,
    tool_call_keys: &'a mut Vec<Option<String>>,
    pending_tool_deltas: &'a mut std::collections::HashMap<String, Vec<String>>,
    tool_calls_started: &'a mut std::collections::HashSet<String>,
}

impl<'a> ToolCallState<'a> {
    fn new(
        tool_calls: &'a mut std::collections::HashMap<String, (String, String)>,
        tool_call_positions: &'a mut std::collections::HashMap<String, usize>,
        content: &'a mut Vec<AssistantContent>,
        tool_call_keys: &'a mut Vec<Option<String>>,
        pending_tool_deltas: &'a mut std::collections::HashMap<String, Vec<String>>,
        tool_calls_started: &'a mut std::collections::HashSet<String>,
    ) -> Self {
        Self {
            tool_calls,
            tool_call_positions,
            content,
            tool_call_keys,
            pending_tool_deltas,
            tool_calls_started,
        }
    }
}

fn append_text_delta(
    content: &mut Vec<AssistantContent>,
    tool_call_keys: &mut Vec<Option<String>>,
    delta: &str,
) {
    if let Some(AssistantContent::Text { text }) = content.last_mut() {
        text.push_str(delta);
    } else {
        content.push(AssistantContent::Text {
            text: delta.to_string(),
        });
        tool_call_keys.push(None);
    }
}

fn append_thinking_delta(
    content: &mut Vec<AssistantContent>,
    tool_call_keys: &mut Vec<Option<String>>,
    delta: &str,
) {
    if let Some(AssistantContent::Thought {
        thought: ThoughtContent::Simple { text },
    }) = content.last_mut()
    {
        text.push_str(delta);
    } else {
        content.push(AssistantContent::Thought {
            thought: ThoughtContent::Simple {
                text: delta.to_string(),
            },
        });
        tool_call_keys.push(None);
    }
}

fn ensure_tool_call_placeholder(
    call_id: &str,
    tool_calls: &mut std::collections::HashMap<String, (String, String)>,
    tool_call_positions: &mut std::collections::HashMap<String, usize>,
    content: &mut Vec<AssistantContent>,
    tool_call_keys: &mut Vec<Option<String>>,
) {
    if tool_call_positions.contains_key(call_id) {
        return;
    }
    let entry = tool_calls
        .entry(call_id.to_string())
        .or_insert_with(|| (String::new(), String::new()));
    let pos = content.len();
    content.push(AssistantContent::ToolCall {
        tool_call: steer_tools::ToolCall {
            id: call_id.to_string(),
            name: entry.0.clone(),
            parameters: serde_json::Value::String(entry.1.clone()),
        },
        thought_signature: None,
    });
    tool_call_keys.push(Some(call_id.to_string()));
    tool_call_positions.insert(call_id.to_string(), pos);
}

fn handle_tool_call_delta(
    data: &serde_json::Value,
    item_to_call_id: &std::collections::HashMap<String, String>,
    tool_state: &mut ToolCallState<'_>,
    delta_keys: &[&str],
) -> ToolDeltaOutcome {
    let Some(delta) = extract_first_non_empty_str(data, delta_keys) else {
        return ToolDeltaOutcome::Missing;
    };
    let Some(resolution) = resolve_tool_call_id_for_event(data, item_to_call_id) else {
        return ToolDeltaOutcome::Missing;
    };
    match resolution {
        ToolCallIdResolution::Resolved(call_id) => {
            let entry = tool_state
                .tool_calls
                .entry(call_id.clone())
                .or_insert_with(|| (String::new(), String::new()));
            entry.1.push_str(&delta);
            ensure_tool_call_placeholder(
                &call_id,
                tool_state.tool_calls,
                tool_state.tool_call_positions,
                tool_state.content,
                tool_state.tool_call_keys,
            );
            if tool_state.tool_calls_started.contains(&call_id) {
                ToolDeltaOutcome::Emit { id: call_id, delta }
            } else {
                tool_state
                    .pending_tool_deltas
                    .entry(call_id)
                    .or_default()
                    .push(delta);
                ToolDeltaOutcome::Buffered
            }
        }
        ToolCallIdResolution::Pending(item_id) => {
            let entry = tool_state
                .tool_calls
                .entry(item_id.clone())
                .or_insert_with(|| (String::new(), String::new()));
            entry.1.push_str(&delta);
            ensure_tool_call_placeholder(
                &item_id,
                tool_state.tool_calls,
                tool_state.tool_call_positions,
                tool_state.content,
                tool_state.tool_call_keys,
            );
            tool_state
                .pending_tool_deltas
                .entry(item_id)
                .or_default()
                .push(delta);
            ToolDeltaOutcome::Buffered
        }
    }
}

fn handle_tool_call_done(
    data: &serde_json::Value,
    item_to_call_id: &std::collections::HashMap<String, String>,
    tool_state: &mut ToolCallState<'_>,
    arg_keys: &[&str],
) {
    let Some(resolution) = resolve_tool_call_id_for_event(data, item_to_call_id) else {
        return;
    };
    let Some(arguments) = extract_first_non_empty_str(data, arg_keys) else {
        return;
    };
    let key = match resolution {
        ToolCallIdResolution::Resolved(call_id) => call_id,
        ToolCallIdResolution::Pending(item_id) => item_id,
    };
    let entry = tool_state
        .tool_calls
        .entry(key.clone())
        .or_insert_with(|| (String::new(), String::new()));
    entry.1 = arguments;
    ensure_tool_call_placeholder(
        &key,
        tool_state.tool_calls,
        tool_state.tool_call_positions,
        tool_state.content,
        tool_state.tool_call_keys,
    );
}

fn register_tool_call(
    call_id: &str,
    name: Option<String>,
    args: Option<String>,
    tool_state: &mut ToolCallState<'_>,
) -> Option<StreamChunk> {
    if call_id.is_empty() {
        return None;
    }
    let mut name_to_emit = None;
    {
        let entry = tool_state
            .tool_calls
            .entry(call_id.to_string())
            .or_insert_with(|| (String::new(), String::new()));
        if let Some(name) = name.filter(|value| !value.is_empty())
            && entry.0.is_empty()
        {
            entry.0 = name;
        }
        if let Some(args) = args.filter(|value| !value.is_empty())
            && entry.1.is_empty()
        {
            entry.1 = args;
        }
        if !entry.0.is_empty() {
            name_to_emit = Some(entry.0.clone());
        }
    }
    ensure_tool_call_placeholder(
        call_id,
        tool_state.tool_calls,
        tool_state.tool_call_positions,
        tool_state.content,
        tool_state.tool_call_keys,
    );
    if let Some(name) = name_to_emit {
        if tool_state.tool_calls_started.insert(call_id.to_string()) {
            return Some(StreamChunk::ToolUseStart {
                id: call_id.to_string(),
                name,
            });
        }
    }
    None
}

fn drop_tool_call_placeholder_at(
    call_id: &str,
    index: usize,
    tool_call_positions: &mut std::collections::HashMap<String, usize>,
    content: &mut Vec<AssistantContent>,
    tool_call_keys: &mut Vec<Option<String>>,
) {
    tool_call_positions.remove(call_id);
    content.remove(index);
    tool_call_keys.remove(index);
    for pos in tool_call_positions.values_mut() {
        if *pos > index {
            *pos -= 1;
        }
    }
}

fn promote_tool_call_id(provisional_id: &str, call_id: &str, tool_state: &mut ToolCallState<'_>) {
    if provisional_id == call_id {
        return;
    }

    if let Some((pending_name, pending_args)) = tool_state.tool_calls.remove(provisional_id) {
        let entry = tool_state
            .tool_calls
            .entry(call_id.to_string())
            .or_insert_with(|| (String::new(), String::new()));
        if entry.0.is_empty() {
            entry.0 = pending_name;
        }
        if entry.1.is_empty() || pending_args.len() > entry.1.len() {
            entry.1 = pending_args;
        }
    }

    if let Some(deltas) = tool_state.pending_tool_deltas.remove(provisional_id) {
        tool_state
            .pending_tool_deltas
            .entry(call_id.to_string())
            .or_default()
            .extend(deltas);
    }

    if tool_state.tool_calls_started.remove(provisional_id) {
        tool_state.tool_calls_started.insert(call_id.to_string());
    }

    for key in tool_state.tool_call_keys.iter_mut() {
        if key.as_deref() == Some(provisional_id) {
            *key = Some(call_id.to_string());
        }
    }

    let provisional_pos = tool_state.tool_call_positions.get(provisional_id).copied();
    let call_pos = tool_state.tool_call_positions.get(call_id).copied();

    match (provisional_pos, call_pos) {
        (Some(p_pos), None) => {
            tool_state.tool_call_positions.remove(provisional_id);
            tool_state
                .tool_call_positions
                .insert(call_id.to_string(), p_pos);
        }
        (Some(p_pos), Some(c_pos)) => match p_pos.cmp(&c_pos) {
            std::cmp::Ordering::Less => {
                drop_tool_call_placeholder_at(
                    call_id,
                    c_pos,
                    tool_state.tool_call_positions,
                    tool_state.content,
                    tool_state.tool_call_keys,
                );
                tool_state.tool_call_positions.remove(provisional_id);
                tool_state
                    .tool_call_positions
                    .insert(call_id.to_string(), p_pos);
            }
            std::cmp::Ordering::Greater => {
                drop_tool_call_placeholder_at(
                    provisional_id,
                    p_pos,
                    tool_state.tool_call_positions,
                    tool_state.content,
                    tool_state.tool_call_keys,
                );
                tool_state
                    .tool_call_positions
                    .insert(call_id.to_string(), c_pos);
            }
            std::cmp::Ordering::Equal => {
                tool_state.tool_call_positions.remove(provisional_id);
                tool_state
                    .tool_call_positions
                    .insert(call_id.to_string(), p_pos);
            }
        },
        (None, Some(c_pos)) => {
            tool_state
                .tool_call_positions
                .insert(call_id.to_string(), c_pos);
        }
        (None, None) => {}
    }
}

fn drain_pending_tool_deltas(
    call_id: &str,
    pending_tool_deltas: &mut std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    pending_tool_deltas.remove(call_id).unwrap_or_default()
}

fn log_request_payload(request: &ResponsesRequest, is_stream: bool) {
    match serde_json::to_string(request) {
        Ok(payload) => {
            if is_stream {
                debug!(target: "openai::responses::stream", "Request payload: {}", payload);
            } else {
                debug!(target: "openai::responses", "Request payload: {}", payload);
            }
        }
        Err(err) => {
            if is_stream {
                debug!(
                    target: "openai::responses::stream",
                    "Failed to serialize request payload: {}",
                    err
                );
            } else {
                debug!(
                    target: "openai::responses",
                    "Failed to serialize request payload: {}",
                    err
                );
            }
        }
    }
}

fn resolve_call_id(
    value: &serde_json::Value,
    item_to_call_id: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if let Some(call_id) = extract_non_empty_str(value, "call_id") {
        return Some(call_id);
    }

    let candidate =
        extract_non_empty_str(value, "item_id").or_else(|| extract_non_empty_str(value, "id"))?;

    if let Some(mapped) = item_to_call_id.get(&candidate) {
        return Some(mapped.clone());
    }

    Some(candidate)
}

fn select_call_id(call_id: Option<String>, id: Option<String>) -> Option<String> {
    match call_id {
        Some(value) if !value.is_empty() => Some(value),
        _ => id.filter(|value| !value.is_empty()),
    }
}

fn select_tool_name(name: Option<String>, tool_name: Option<String>) -> Option<String> {
    match name {
        Some(value) if !value.is_empty() => Some(value),
        _ => tool_name.filter(|value| !value.is_empty()),
    }
}

fn parse_tool_parameters(tool_name: &str, raw_args: &str) -> serde_json::Value {
    if raw_args.trim().is_empty() {
        return serde_json::Value::Object(serde_json::Map::new());
    }
    match serde_json::from_str(raw_args) {
        Ok(params) => params,
        Err(e) => {
            tracing::warn!(
                target: "openai::responses",
                "Failed to parse tool arguments for '{}': {}. Raw arguments: {}",
                tool_name,
                e,
                raw_args
            );
            serde_json::Value::Object(serde_json::Map::new())
        }
    }
}

fn parse_http_error_response(status_code: u16, body_text: String) -> ApiError {
    match serde_json::from_str::<ResponsesHttpErrorEnvelope>(&body_text) {
        Ok(envelope) => {
            if let Some(error) = envelope.error {
                let details = stream_error_message(&error);
                return ApiError::ServerError {
                    provider: "openai".to_string(),
                    status_code,
                    details,
                };
            }

            ApiError::ServerError {
                provider: "openai".to_string(),
                status_code,
                details: body_text,
            }
        }
        Err(_) => ApiError::ServerError {
            provider: "openai".to_string(),
            status_code,
            details: body_text,
        },
    }
}

fn stream_error_message(error: &ResponseError) -> String {
    let message = error.message.trim();
    let base = if message.is_empty() {
        format!("OpenAI response error ({})", error.code)
    } else {
        message.to_string()
    };

    match &error.param {
        Some(param) if !param.is_empty() => format!("{} (param: {param})", base),
        _ => base,
    }
}

fn stream_error_details(error: &ResponseError) -> (String, String) {
    let error_type = error
        .error_type
        .clone()
        .unwrap_or_else(|| error.code.clone());
    let message = stream_error_message(error);
    (error_type, message)
}

fn auth_header_context(model_id: &ModelId, request_kind: RequestKind) -> AuthHeaderContext {
    AuthHeaderContext {
        model_id: Some(AuthModelId {
            provider_id: AuthProviderId(model_id.provider.as_str().to_string()),
            model_id: model_id.id.clone(),
        }),
        request_kind,
    }
}

fn is_auth_status(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

fn truncate_body(body: &str) -> String {
    const LIMIT: usize = 512;
    let mut chars = body.chars();
    let truncated: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn apply_instruction_policy(
    system: Option<SystemContext>,
    policy: Option<&InstructionPolicy>,
) -> Option<String> {
    let base = system.as_ref().and_then(|context| {
        let trimmed = context.prompt.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let context = system
        .as_ref()
        .and_then(|context| context.render_with_prompt(base.clone()));

    match policy {
        None => context,
        Some(InstructionPolicy::Prefix(prefix)) => {
            if let Some(context) = context {
                Some(format!("{prefix}\n{context}"))
            } else {
                Some(prefix.clone())
            }
        }
        Some(InstructionPolicy::DefaultIfEmpty(default)) => {
            if context.is_some() {
                context
            } else {
                Some(default.clone())
            }
        }
        Some(InstructionPolicy::Override(override_text)) => {
            let mut combined = override_text.clone();
            if let Some(system) = system.as_ref() {
                let overlay = system.prompt.trim();
                if !overlay.is_empty() {
                    combined.push_str("\n\n## Operating Mode\n");
                    combined.push_str(overlay);
                }

                let env = system
                    .environment
                    .as_ref()
                    .map(|env| env.as_context())
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());
                if let Some(env) = env {
                    combined.push_str("\n\n");
                    combined.push_str(&env);
                }
            }
            Some(combined)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::SystemContext;
    use crate::app::conversation::{AssistantContent, Message, MessageData, UserContent};
    use crate::workspace::EnvironmentInfo;

    use schemars::schema_for;
    use steer_tools::ToolSchema;
    use steer_tools::tools::dispatch_agent::DispatchAgentParams;

    #[test]
    fn test_responses_api_message_conversion() {
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

        let actual = Client::convert_messages_to_input(&messages);
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
        let client = Client::new("test_key".to_string()).expect("openai responses client");

        let tools = vec![ToolSchema {
            name: "get_weather".to_string(),
            display_name: "Get Weather".to_string(),
            description: "Get the weather".to_string(),
            input_schema: steer_tools::InputSchema::object(
                serde_json::json!({
                    "location": {
                        "type": "string",
                        "description": "City name"
                    }
                })
                .as_object()
                .unwrap()
                .clone(),
                vec!["location".to_string()],
            ),
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

        let model_id = ModelId::new(crate::config::provider::openai(), "gpt-4.1-2025-04-14");
        let request = client.build_request(
            &model_id,
            messages,
            Some(SystemContext::new(
                "You are a weather assistant".to_string(),
            )),
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
    fn test_responses_api_dispatch_agent_schema_includes_target() {
        let client = Client::new("test_key".to_string()).expect("openai responses client");

        let input_schema: steer_tools::InputSchema = schema_for!(DispatchAgentParams).into();
        let tools = vec![ToolSchema {
            name: "dispatch_agent".to_string(),
            display_name: "Dispatch Agent".to_string(),
            description: "Dispatch agent".to_string(),
            input_schema,
        }];

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Launch an agent".to_string(),
                }],
            },
            timestamp: 1000,
            id: "msg1".to_string(),
            parent_message_id: None,
        }];

        let model_id = ModelId::new(crate::config::provider::openai(), "gpt-4.1-2025-04-14");
        let request = client.build_request(&model_id, messages, None, Some(tools), None);

        let tool = request
            .tools
            .expect("expected tools")
            .into_iter()
            .next()
            .unwrap();
        let parameters = tool.parameters.clone();
        let summary = steer_tools::InputSchema::from(parameters.clone()).summary();

        assert!(summary.properties.contains_key("prompt"));
        assert!(summary.properties.contains_key("target"));
        assert!(summary.required.contains(&"prompt".to_string()));
        assert!(summary.required.contains(&"target".to_string()));
    }

    #[test]
    fn test_override_appends_overlay_and_env() {
        let env = EnvironmentInfo {
            working_directory: std::path::PathBuf::from("/tmp"),
            vcs: None,
            platform: "linux".to_string(),
            date: "2025-01-01".to_string(),
            directory_structure: String::new(),
            readme_content: None,
            memory_file_name: None,
            memory_file_content: None,
        };

        let system = SystemContext::with_environment("Custom prompt".to_string(), Some(env));
        let rendered = apply_instruction_policy(
            Some(system),
            Some(&InstructionPolicy::Override("Override".to_string())),
        )
        .expect("expected rendered instructions");
        let expected = "Override\n\n## Operating Mode\nCustom prompt\n\nHere is useful information about the environment you are running in:\n<env>\nWorking directory: /tmp\nVCS: none\nPlatform: linux\nToday's date: 2025-01-01\n</env>";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn test_responses_api_reasoning_config() {
        let client = Client::new("test_key".to_string()).expect("openai responses client");

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
        let model_id = ModelId::new(crate::config::provider::openai(), "codex-mini-latest");
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
        let model_id = ModelId::new(crate::config::provider::openai(), "gpt-4.1-2025-04-14");
        let request = client.build_request(&model_id, messages, None, None, None);

        assert!(request.reasoning.is_none());
    }

    #[test]
    fn test_responses_api_tool_result_conversion() {
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
                        thought_signature: None,
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

        let actual = Client::convert_messages_to_input(&messages);
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
        // Test parsing function call output
        let output = vec![ResponseOutputItem::FunctionCall {
            id: "fc_123".to_string(),
            call_id: Some("call_456".to_string()),
            name: "get_weather".to_string(),
            arguments: r#"{"location":"Boston"}"#.to_string(),
            status: "completed".to_string(),
        }];

        let actual = Client::convert_output(Some(output));
        let expected = vec![AssistantContent::ToolCall {
            tool_call: steer_tools::ToolCall {
                id: "call_456".to_string(),
                name: "get_weather".to_string(),
                parameters: serde_json::json!({"location": "Boston"}),
            },
            thought_signature: None,
        }];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_output_refusal_parsing() {
        let output = vec![ResponseOutputItem::Message {
            id: "msg_123".to_string(),
            status: "completed".to_string(),
            role: "assistant".to_string(),
            content: vec![MessageContentPart::Refusal {
                refusal: "I can't help with that.".to_string(),
            }],
        }];

        let actual = Client::convert_output(Some(output));
        let expected = vec![AssistantContent::Text {
            text: "I can't help with that.".to_string(),
        }];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_output_custom_tool_call_parsing() {
        let output = vec![ResponseOutputItem::CustomToolCall {
            id: Some("ctc_123".to_string()),
            call_id: Some("call_456".to_string()),
            name: Some("do_thing".to_string()),
            tool_name: None,
            input: Some(r#"{"value":42}"#.to_string()),
            status: Some("completed".to_string()),
            extra: Default::default(),
        }];

        let actual = Client::convert_output(Some(output));
        let expected = vec![AssistantContent::ToolCall {
            tool_call: steer_tools::ToolCall {
                id: "call_456".to_string(),
                name: "do_thing".to_string(),
                parameters: serde_json::json!({"value": 42}),
            },
            thought_signature: None,
        }];

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_responses_api_reasoning_extraction() {
        let response = ResponsesApiResponse {
            id: "resp_123".to_string(),
            object: "response".to_string(),
            created_at: 1_234_567_890,
            status: "completed".to_string(),
            error: None,
            incomplete_details: None,
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

        let actual = Client::convert_response(response);
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

    #[test]
    fn test_parse_http_error_response_uses_typed_error_message() {
        let parsed = parse_http_error_response(
            500,
            r#"{"error":{"code":"server_error","message":"backend exploded","param":"model"}}"#
                .to_string(),
        );

        match parsed {
            ApiError::ServerError {
                status_code,
                details,
                ..
            } => {
                assert_eq!(status_code, 500);
                assert_eq!(details, "backend exploded (param: model)");
            }
            other => panic!("Expected server error, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_http_error_response_falls_back_to_raw_body() {
        let body = "upstream unavailable".to_string();
        let parsed = parse_http_error_response(503, body.clone());

        match parsed {
            ApiError::ServerError {
                status_code,
                details,
                ..
            } => {
                assert_eq!(status_code, 503);
                assert_eq!(details, body);
            }
            other => panic!("Expected server error, got {other:?}"),
        }
    }

    #[test]
    fn test_stream_error_message_uses_code_when_message_missing() {
        let error = ResponseError {
            code: "rate_limit_exceeded".to_string(),
            message: " ".to_string(),
            param: None,
            error_type: None,
            extra: Default::default(),
        };

        let message = stream_error_message(&error);
        assert_eq!(message, "OpenAI response error (rate_limit_exceeded)");
    }

    #[tokio::test]
    async fn test_convert_responses_stream_error_event_prefers_typed_fields() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("error".to_string()),
            data: r#"{"type":"error","code":"rate_limit_exceeded","message":"Too many requests","param":"model","sequence_number":4}"#.to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let error_chunk = stream.next().await.unwrap();
        assert!(matches!(
            error_chunk,
            StreamChunk::Error(StreamError::Provider {
                ref provider,
                ref error_type,
                ref message,
            }) if provider == "openai" && error_type == "error" && message == "Too many requests"
        ));
    }

    #[tokio::test]
    async fn test_convert_responses_stream_failed_event_uses_response_error() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("response.failed".to_string()),
            data: r#"{"type":"response.failed","sequence_number":9,"response":{"id":"resp_1","object":"response","created_at":1,"status":"failed","error":{"code":"invalid_prompt","message":"Prompt blocked","param":"input"}}}"#.to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let error_chunk = stream.next().await.unwrap();
        assert!(matches!(
            error_chunk,
            StreamChunk::Error(StreamError::Provider {
                ref provider,
                ref error_type,
                ref message,
            }) if provider == "openai" && error_type == "invalid_prompt" && message == "Prompt blocked (param: input)"
        ));
    }

    #[tokio::test]
    async fn test_convert_responses_stream_failed_event_without_error_object() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("response.failed".to_string()),
            data: r#"{"type":"response.failed","sequence_number":10,"response":{"id":"resp_2","object":"response","created_at":1,"status":"failed"}}"#.to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let error_chunk = stream.next().await.unwrap();
        assert!(matches!(
            error_chunk,
            StreamChunk::Error(StreamError::Provider {
                ref provider,
                ref error_type,
                ref message,
            }) if provider == "openai" && error_type == "response_failed" && message == "Response failed without an error object"
        ));
    }

    #[tokio::test]
    async fn test_convert_responses_stream_error_event_fallback_on_invalid_payload() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("error".to_string()),
            data: "not-json".to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let error_chunk = stream.next().await.unwrap();
        assert!(matches!(
            error_chunk,
            StreamChunk::Error(StreamError::Provider {
                ref provider,
                ref error_type,
                ref message,
            }) if provider == "openai" && error_type == "stream_error" && message == "not-json"
        ));
    }

    #[tokio::test]
    async fn test_convert_responses_stream_failed_event_fallback_on_invalid_payload() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![Ok(SseEvent {
            event_type: Some("response.failed".to_string()),
            data: "not-json".to_string(),
            id: None,
        })];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let error_chunk = stream.next().await.unwrap();
        assert!(matches!(
            error_chunk,
            StreamChunk::Error(StreamError::Provider {
                ref provider,
                ref error_type,
                ref message,
            }) if provider == "openai" && error_type == "response_failed" && message == "not-json"
        ));
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
                data: r"{}".to_string(),
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
                data: r"{}".to_string(),
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
                data: r"{}".to_string(),
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
            if let AssistantContent::ToolCall { tool_call, .. } = &response.content[0] {
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
    async fn test_convert_responses_stream_with_custom_tool_call() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.output_item.added".to_string()),
                data: r#"{"item":{"type":"custom_tool_call","id":"item_1","call_id":"call_123","name":"do_thing","input":""}}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.custom_tool_call_input.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"{\"value\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.custom_tool_call_input.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"42}"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r"{}".to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let tool_start = stream.next().await.unwrap();
        assert!(
            matches!(tool_start, StreamChunk::ToolUseStart { ref id, ref name } if id == "call_123" && name == "do_thing")
        );

        let arg_delta_1 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_1, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "{\"value\":")
        );

        let arg_delta_2 = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta_2, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "42}")
        );

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            if let AssistantContent::ToolCall { tool_call, .. } = &response.content[0] {
                assert_eq!(tool_call.id, "call_123");
                assert_eq!(tool_call.name, "do_thing");
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
                data: r#"{"item_id":"item_1","delta":"{\"city\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"item_id":"item_1","delta":"\"NYC\"}"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r"{}".to_string(),
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
            if let AssistantContent::ToolCall { tool_call, .. } = &response.content[0] {
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
    async fn test_convert_responses_stream_buffers_deltas_until_output_item_added() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"item_id":"item_1","delta":"{\"city\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.output_item.added".to_string()),
                data: r#"{"item":{"type":"function_call","id":"item_1","call_id":"call_123","name":"get_weather"}}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"item_id":"item_1","delta":"\"NYC\"}"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r"{}".to_string(),
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
            if let AssistantContent::ToolCall { tool_call, .. } = &response.content[0] {
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
    async fn test_convert_responses_stream_buffers_deltas_until_tool_start() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.function_call_arguments.delta".to_string()),
                data: r#"{"call_id":"call_123","delta":"{\"city\":"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.function_call.created".to_string()),
                data: r#"{"call_id":"call_123","name":"get_weather"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r"{}".to_string(),
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

        let arg_delta = stream.next().await.unwrap();
        assert!(
            matches!(arg_delta, StreamChunk::ToolUseInputDelta { ref id, ref delta } if id == "call_123" && delta == "{\"city\":")
        );

        let complete = stream.next().await.unwrap();
        assert!(matches!(complete, StreamChunk::MessageComplete(_)));
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

    #[tokio::test]
    async fn test_convert_responses_stream_with_refusal_delta() {
        use crate::api::sse::SseEvent;
        use futures::stream;
        use std::pin::pin;

        let events = vec![
            Ok(SseEvent {
                event_type: Some("response.refusal.delta".to_string()),
                data: r#"{"delta":"No"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.refusal.delta".to_string()),
                data: r#"{"delta":" thanks"}"#.to_string(),
                id: None,
            }),
            Ok(SseEvent {
                event_type: Some("response.completed".to_string()),
                data: r"{}".to_string(),
                id: None,
            }),
        ];

        let sse_stream = stream::iter(events);
        let token = CancellationToken::new();
        let mut stream = pin!(Client::convert_responses_stream(sse_stream, token));

        let first_delta = stream.next().await.unwrap();
        assert!(matches!(first_delta, StreamChunk::TextDelta(ref t) if t == "No"));

        let second_delta = stream.next().await.unwrap();
        assert!(matches!(second_delta, StreamChunk::TextDelta(ref t) if t == " thanks"));

        let complete = stream.next().await.unwrap();
        if let StreamChunk::MessageComplete(response) = complete {
            assert_eq!(response.content.len(), 1);
            assert!(
                matches!(&response.content[0], AssistantContent::Text { text } if text == "No thanks")
            );
        } else {
            panic!("Expected MessageComplete");
        }
    }
}
