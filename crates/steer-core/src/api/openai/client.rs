use super::OpenAIMode;
use super::chat;
use super::responses;
use crate::api::error::ApiError;
use crate::api::provider::{CompletionResponse, CompletionStream, Provider};
use crate::app::SystemContext;
use crate::app::conversation::Message;
use crate::auth::OpenAiResponsesAuth;
use crate::config::model::{ModelId, ModelParameters};
use async_trait::async_trait;
use steer_tools::ToolSchema;
use tokio_util::sync::CancellationToken;

/// Unified OpenAI client that supports both the Chat and Responses APIs.
///
/// The client internally manages two separate clients for the different API modes
/// and delegates requests based on the configured default mode.
pub struct OpenAIClient {
    responses_client: responses::Client,
    chat_client: Option<chat::Client>,
    default_mode: OpenAIMode,
}

impl OpenAIClient {
    /// Create a new OpenAI client with a specific mode.
    pub fn with_mode(api_key: String, mode: OpenAIMode) -> Self {
        Self {
            responses_client: responses::Client::new(api_key.clone()),
            chat_client: Some(chat::Client::new(api_key)),
            default_mode: mode,
        }
    }

    /// Create a new OpenAI client with a custom base URL and mode.
    pub fn with_base_url_mode(api_key: String, base_url: Option<String>, mode: OpenAIMode) -> Self {
        Self {
            responses_client: responses::Client::with_base_url(api_key.clone(), base_url.clone()),
            chat_client: Some(chat::Client::with_base_url(api_key, base_url)),
            default_mode: mode,
        }
    }

    pub fn with_directive(directive: OpenAiResponsesAuth, base_url: Option<String>) -> Self {
        Self {
            responses_client: responses::Client::with_directive(directive, base_url),
            chat_client: None,
            default_mode: OpenAIMode::Responses,
        }
    }
}

#[async_trait]
impl Provider for OpenAIClient {
    fn name(&self) -> &'static str {
        "openai"
    }

    async fn complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        match self.default_mode {
            OpenAIMode::Responses => {
                self.responses_client
                    .complete(model_id, messages, system, tools, call_options, token)
                    .await
            }
            OpenAIMode::Chat => {
                let chat_client = self.chat_client.as_ref().ok_or_else(|| {
                    ApiError::Configuration(
                        "OpenAI chat mode is not available with OAuth authentication".to_string(),
                    )
                })?;
                chat_client
                    .complete(model_id, messages, system, tools, call_options, token)
                    .await
            }
        }
    }

    async fn stream_complete(
        &self,
        model_id: &ModelId,
        messages: Vec<Message>,
        system: Option<SystemContext>,
        tools: Option<Vec<ToolSchema>>,
        call_options: Option<ModelParameters>,
        token: CancellationToken,
    ) -> Result<CompletionStream, ApiError> {
        match self.default_mode {
            OpenAIMode::Responses => {
                self.responses_client
                    .stream_complete(model_id, messages, system, tools, call_options, token)
                    .await
            }
            OpenAIMode::Chat => {
                let chat_client = self.chat_client.as_ref().ok_or_else(|| {
                    ApiError::Configuration(
                        "OpenAI chat mode is not available with OAuth authentication".to_string(),
                    )
                })?;
                chat_client
                    .stream_complete(model_id, messages, system, tools, call_options, token)
                    .await
            }
        }
    }
}
