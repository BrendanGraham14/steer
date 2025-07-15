use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::provider::{CompletionResponse, Provider};
use crate::app::conversation::Message as AppMessage;
use steer_tools::ToolSchema;

use super::{OpenAIMode, chat, responses};

/// Unified OpenAI client that routes to the appropriate API based on configured mode
#[derive(Clone)]
#[allow(dead_code)]
pub struct OpenAIClient {
    chat_client: chat::Client,
    responses_client: responses::Client,
    default_mode: OpenAIMode,
}

impl OpenAIClient {
    pub fn new(api_key: String) -> Self {
        Self::with_mode(api_key, OpenAIMode::Responses)
    }

    pub fn with_mode(api_key: String, mode: OpenAIMode) -> Self {
        Self {
            chat_client: chat::Client::new(api_key.clone()),
            responses_client: responses::Client::new(api_key),
            default_mode: mode,
        }
    }

    pub fn with_base_url_mode(api_key: String, base_url: Option<String>, mode: OpenAIMode) -> Self {
        Self {
            chat_client: chat::Client::with_base_url(api_key.clone(), base_url.clone()),
            responses_client: responses::Client::with_base_url(api_key, base_url),
            default_mode: mode,
        }
    }
}

#[async_trait]
impl Provider for OpenAIClient {
    fn name(&self) -> &'static str {
        super::PROVIDER_NAME
    }

    async fn complete(
        &self,
        model: Model,
        messages: Vec<AppMessage>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse, ApiError> {
        match self.default_mode {
            OpenAIMode::Responses => {
                self.responses_client
                    .complete(
                        model,
                        messages.clone(),
                        system.clone(),
                        tools.clone(),
                        token.clone(),
                    )
                    .await
                // Optional fallback to chat if Responses says unsupported
                // .or_else(|e| match e {
                //     ApiError::InvalidRequest { details, .. } if details.contains("not supported") => {
                //         self.chat_client.complete(model, messages, system, tools, token)
                //     }
                //     err => Err(err),
                // })
            }
            OpenAIMode::Chat => {
                self.chat_client
                    .complete(model, messages, system, tools, token)
                    .await
            }
        }
    }
}
