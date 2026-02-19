use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::api::provider::{CompletionResponse, StreamChunk};
use crate::app::SystemContext;
use crate::app::conversation::Message;
use crate::app::domain::action::{ModelCallError, ModelCallRequestErrorKind};
use crate::app::domain::delta::{StreamDelta, ToolCallDelta};
use crate::app::domain::types::{MessageId, OpId, SessionId, ToolCallId};
use crate::config::model::ModelId;
use crate::tools::{SessionMcpBackends, ToolExecutor};
use steer_tools::{ToolCall, ToolError, ToolResult, ToolSchema};

#[derive(Clone)]
pub struct EffectInterpreter {
    api_client: Arc<ApiClient>,
    tool_executor: Arc<ToolExecutor>,
    session_id: Option<SessionId>,
    session_backends: Option<Arc<SessionMcpBackends>>,
}

pub(crate) struct DeltaStreamContext {
    tx: mpsc::Sender<StreamDelta>,
    context: (OpId, MessageId),
}

impl DeltaStreamContext {
    pub(crate) fn new(tx: mpsc::Sender<StreamDelta>, context: (OpId, MessageId)) -> Self {
        Self { tx, context }
    }
}

impl EffectInterpreter {
    pub fn new(api_client: Arc<ApiClient>, tool_executor: Arc<ToolExecutor>) -> Self {
        Self {
            api_client,
            tool_executor,
            session_id: None,
            session_backends: None,
        }
    }

    pub fn with_session(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn with_session_backends(mut self, backends: Arc<SessionMcpBackends>) -> Self {
        self.session_backends = Some(backends);
        self
    }

    pub fn model_context_window_tokens(&self, model: &ModelId) -> Option<u32> {
        self.api_client.model_context_window_tokens(model)
    }

    pub fn model_max_output_tokens(&self, model: &ModelId) -> Option<u32> {
        self.api_client.model_max_output_tokens(model)
    }

    pub async fn call_model(
        &self,
        model: ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        tools: Vec<ToolSchema>,
        cancel_token: CancellationToken,
    ) -> Result<CompletionResponse, ModelCallError> {
        self.call_model_with_deltas(model, messages, system_context, tools, cancel_token, None)
            .await
    }

    pub(crate) async fn call_model_with_deltas(
        &self,
        model: ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        tools: Vec<ToolSchema>,
        cancel_token: CancellationToken,
        delta_stream: Option<DeltaStreamContext>,
    ) -> Result<CompletionResponse, ModelCallError> {
        let tools_option = if tools.is_empty() { None } else { Some(tools) };

        let mut stream = self
            .api_client
            .stream_complete(
                &model,
                messages,
                system_context,
                tools_option,
                None,
                cancel_token,
            )
            .await
            .map_err(|error| ModelCallError::RequestStartFailed {
                kind: ModelCallRequestErrorKind::from_api_error(&error),
                message: error.to_string(),
            })?;

        let mut final_response = None;
        while let Some(chunk) = stream.next().await {
            match chunk {
                StreamChunk::TextDelta(text) => {
                    if let Some(delta_stream) = &delta_stream {
                        let (op_id, message_id) = &delta_stream.context;
                        let delta = StreamDelta::TextChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            delta: text,
                        };
                        let _ = delta_stream.tx.send(delta).await;
                    }
                }
                StreamChunk::ThinkingDelta(thinking) => {
                    if let Some(delta_stream) = &delta_stream {
                        let (op_id, message_id) = &delta_stream.context;
                        let delta = StreamDelta::ThinkingChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            delta: thinking,
                        };
                        let _ = delta_stream.tx.send(delta).await;
                    }
                }
                StreamChunk::ToolUseInputDelta { id, delta } => {
                    if let Some(delta_stream) = &delta_stream {
                        let (op_id, message_id) = &delta_stream.context;
                        let delta = StreamDelta::ToolCallChunk {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                            tool_call_id: ToolCallId::from_string(&id),
                            delta: ToolCallDelta::ArgumentChunk(delta),
                        };
                        let _ = delta_stream.tx.send(delta).await;
                    }
                }
                StreamChunk::Reset => {
                    if let Some(delta_stream) = &delta_stream {
                        let (op_id, message_id) = &delta_stream.context;
                        let delta = StreamDelta::Reset {
                            op_id: *op_id,
                            message_id: message_id.clone(),
                        };
                        let _ = delta_stream.tx.send(delta).await;
                    }
                }
                StreamChunk::MessageComplete(response) => {
                    final_response = Some(response);
                }
                StreamChunk::Error(err) => {
                    return Err(ModelCallError::StreamFailed(err));
                }
                StreamChunk::ToolUseStart { .. } | StreamChunk::ContentBlockStop { .. } => {}
            }
        }

        final_response.ok_or(ModelCallError::MissingCompletionResponse)
    }

    pub async fn execute_tool(
        &self,
        tool_call: ToolCall,
        invoking_model: Option<ModelId>,
        cancel_token: CancellationToken,
    ) -> Result<ToolResult, ToolError> {
        let resolver = self
            .session_backends
            .as_ref()
            .map(|b| b.as_ref() as &dyn crate::tools::BackendResolver);

        if let Some(session_id) = self.session_id {
            self.tool_executor
                .execute_tool_with_session_resolver(
                    &tool_call,
                    session_id,
                    invoking_model,
                    cancel_token,
                    resolver,
                )
                .await
        } else {
            self.tool_executor
                .execute_tool_with_resolver(&tool_call, cancel_token, resolver)
                .await
        }
    }

    pub async fn get_tool_schemas(&self) -> Vec<ToolSchema> {
        let resolver = self
            .session_backends
            .as_ref()
            .map(|b| b.as_ref() as &dyn crate::tools::BackendResolver);

        self.tool_executor
            .get_tool_schemas_with_resolver(resolver)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::{ApiError, StreamError};
    use crate::api::provider::{CompletionResponse, Provider, TokenUsage};
    use crate::app::conversation::AssistantContent;
    use crate::app::validation::ValidatorRegistry;
    use crate::auth::ProviderRegistry;
    use crate::config::model::{ModelId, ModelParameters};
    use crate::config::provider::ProviderId;
    use crate::model_registry::ModelRegistry;
    use crate::tools::BackendRegistry;
    use async_trait::async_trait;

    #[derive(Clone)]
    struct StreamErrorProvider;

    #[async_trait]
    impl Provider for StreamErrorProvider {
        fn name(&self) -> &'static str {
            "stream-error"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "unused".to_string(),
                }],
                usage: None,
            })
        }

        async fn stream_complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<crate::api::provider::CompletionStream, ApiError> {
            Ok(Box::pin(futures_util::stream::once(async {
                StreamChunk::Error(StreamError::Provider {
                    provider: "stream-error".to_string(),
                    kind: crate::api::ProviderStreamErrorKind::StreamError,
                    raw_error_type: Some("stream_error".to_string()),
                    message: "stream failed".to_string(),
                })
            })))
        }
    }

    #[derive(Clone)]
    struct StubProvider;

    #[async_trait]
    impl Provider for StubProvider {
        fn name(&self) -> &'static str {
            "stub"
        }

        async fn complete(
            &self,
            _model_id: &ModelId,
            _messages: Vec<Message>,
            _system: Option<SystemContext>,
            _tools: Option<Vec<ToolSchema>>,
            _call_options: Option<ModelParameters>,
            _token: CancellationToken,
        ) -> Result<CompletionResponse, ApiError> {
            Ok(CompletionResponse {
                content: vec![AssistantContent::Text {
                    text: "ok".to_string(),
                }],
                usage: Some(TokenUsage::new(5, 7, 12)),
            })
        }
    }

    async fn create_test_deps() -> (Arc<ApiClient>, Arc<ToolExecutor>) {
        let model_registry = Arc::new(ModelRegistry::load(&[]).expect("model registry"));
        let provider_registry = Arc::new(ProviderRegistry::load(&[]).expect("provider registry"));
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider().unwrap(),
            provider_registry,
            model_registry,
        ));

        let tool_executor = Arc::new(ToolExecutor::with_components(
            Arc::new(BackendRegistry::new()),
            Arc::new(ValidatorRegistry::new()),
        ));

        (api_client, tool_executor)
    }

    #[tokio::test]
    async fn call_model_preserves_completion_usage() {
        let (api_client, tool_executor) = create_test_deps().await;
        let provider_id = ProviderId("stub".to_string());
        api_client.insert_test_provider(provider_id.clone(), Arc::new(StubProvider));

        let interpreter = EffectInterpreter::new(api_client, tool_executor);
        let result = interpreter
            .call_model(
                ModelId::new(provider_id, "stub-model"),
                vec![],
                None,
                vec![],
                CancellationToken::new(),
            )
            .await
            .expect("model call should succeed");

        assert_eq!(result.usage, Some(TokenUsage::new(5, 7, 12)));
        assert!(matches!(
            result.content.as_slice(),
            [AssistantContent::Text { text }] if text == "ok"
        ));
    }

    #[tokio::test]
    async fn call_model_returns_typed_stream_error() {
        let (api_client, tool_executor) = create_test_deps().await;
        let provider_id = ProviderId("stream-error".to_string());
        api_client.insert_test_provider(provider_id.clone(), Arc::new(StreamErrorProvider));

        let interpreter = EffectInterpreter::new(api_client, tool_executor);
        let error = interpreter
            .call_model(
                ModelId::new(provider_id, "stream-error-model"),
                vec![],
                None,
                vec![],
                CancellationToken::new(),
            )
            .await
            .expect_err("model call should fail when stream emits error");

        assert!(matches!(
            error,
            ModelCallError::StreamFailed(StreamError::Provider { .. })
        ));
    }
}
