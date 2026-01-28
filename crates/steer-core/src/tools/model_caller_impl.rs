use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::api::Client as ApiClient;
use crate::app::SystemContext;
use crate::app::conversation::{Message, MessageData};
use crate::config::model::ModelId;
use crate::tools::services::{ModelCallError, ModelCaller};

pub struct DefaultModelCaller {
    api_client: Arc<ApiClient>,
}

impl DefaultModelCaller {
    pub fn new(api_client: Arc<ApiClient>) -> Self {
        Self { api_client }
    }
}

#[async_trait]
impl ModelCaller for DefaultModelCaller {
    async fn call(
        &self,
        model: &ModelId,
        messages: Vec<Message>,
        system_context: Option<SystemContext>,
        cancel_token: CancellationToken,
    ) -> Result<Message, ModelCallError> {
        let response = self
            .api_client
            .complete(model, messages, system_context, None, None, cancel_token)
            .await
            .map_err(|e| ModelCallError::Api(e.to_string()))?;

        let timestamp = Message::current_timestamp();
        Ok(Message {
            timestamp,
            id: Message::generate_id("assistant", timestamp),
            parent_message_id: None,
            data: MessageData::Assistant {
                content: response.content,
            },
        })
    }
}
