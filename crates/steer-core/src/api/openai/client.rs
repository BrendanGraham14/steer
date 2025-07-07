use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Model;
use crate::api::error::ApiError;
use crate::api::provider::{CompletionResponse, Provider};
use crate::app::conversation::Message as AppMessage;
use steer_tools::ToolSchema;

use super::chat;
use super::responses;

/// Unified OpenAI client that routes to the appropriate API based on model
#[derive(Clone)]
#[allow(dead_code)]
pub struct OpenAIClient {
    chat_client: chat::Client,
    responses_client: responses::Client,
}

impl OpenAIClient {
    pub fn new(api_key: String) -> Self {
        Self {
            chat_client: chat::Client::new(api_key.clone()),
            responses_client: responses::Client::new(api_key),
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
        // Route all OpenAI models to the Responses API by default
        self.responses_client
            .complete(model, messages, system, tools, token)
            .await

        // Optional: Add fallback to chat API if responses fails with "not supported"
        // match self.responses_client.complete(model, messages.clone(), system.clone(), tools.clone(), token.clone()).await {
        //     Err(ApiError::InvalidRequest { details, .. }) if details.contains("not supported") => {
        //         self.chat_client.complete(model, messages, system, tools, token).await
        //     }
        //     result => result,
        // }
    }
}
