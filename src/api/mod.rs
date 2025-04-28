pub mod claude;
pub mod messages;
pub mod openai;
pub mod provider;
pub mod tools;

pub use claude::AnthropicClient;
pub use messages::Message;
pub use openai::OpenAIClient;
pub use provider::{CompletionResponse, ContentBlock, Provider};
pub use tools::{InputSchema, Tool, ToolCall};

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::LlmConfig;

// Enum to represent the different LLM providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
}

// Enum for specific LLM models
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Model {
    Claude3_7Sonnet20250219,
    Claude3_5Haiku20241022,
    Gpt4_1_20250414,
    Gpt4_1Mini20250414,
    Gpt4_1Nano20250414,
    O3_20250416,
}

impl Model {
    pub fn provider(&self) -> ProviderKind {
        match self {
            Model::Claude3_7Sonnet20250219 => ProviderKind::Anthropic,
            Model::Claude3_5Haiku20241022 => ProviderKind::Anthropic,
            Model::Gpt4_1_20250414 => ProviderKind::OpenAI,
            Model::Gpt4_1Mini20250414 => ProviderKind::OpenAI,
            Model::Gpt4_1Nano20250414 => ProviderKind::OpenAI,
            Model::O3_20250416 => ProviderKind::OpenAI,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Model::Claude3_7Sonnet20250219 => "claude-3-7-sonnet-20250219",
            Model::Claude3_5Haiku20241022 => "claude-3-5-haiku-20241022",
            Model::Gpt4_1_20250414 => "gpt-4.1-2025-04-14",
            Model::Gpt4_1Mini20250414 => "gpt-4.1-mini-2025-04-14",
            Model::Gpt4_1Nano20250414 => "gpt-4.1-nano-2025-04-14",
            Model::O3_20250416 => "o3-2025-04-16",
        }
    }
}

// Client struct to act as a facade for multiple providers/models
#[derive(Clone)]
pub struct Client {
    provider_map: Arc<RwLock<HashMap<Model, Arc<dyn Provider>>>>,
    config: LlmConfig,
}

impl Client {
    pub fn new(cfg: &LlmConfig) -> Self {
        Self {
            provider_map: Arc::new(RwLock::new(HashMap::new())),
            config: cfg.clone(),
        }
    }

    fn get_or_create_provider(&self, model: Model) -> Result<Arc<dyn Provider>> {
        // Try read lock first
        if let Some(provider) = self.provider_map.read().unwrap().get(&model) {
            return Ok(provider.clone());
        }

        // If not found, acquire write lock
        let mut map = self.provider_map.write().unwrap();
        // Check again in case another thread added it
        if let Some(provider) = map.get(&model) {
            return Ok(provider.clone());
        }

        // If still not found, create and insert
        let provider_kind = model.provider();
        let key = self.config.key_for(provider_kind).ok_or_else(|| {
            anyhow!(
                "API key missing for {:?} needed by model {:?}",
                provider_kind,
                model
            )
        })?;
        let provider_instance: Arc<dyn Provider> = match provider_kind {
            ProviderKind::Anthropic => Arc::new(AnthropicClient::new(key)),
            ProviderKind::OpenAI => Arc::new(OpenAIClient::new(key)),
        };
        map.insert(model, provider_instance.clone());
        Ok(provider_instance)
    }

    pub async fn complete(
        &self,
        model: Model,
        messages: Vec<messages::Message>,
        system: Option<String>,
        tools: Option<Vec<Tool>>,
        token: CancellationToken,
    ) -> Result<CompletionResponse> {
        let provider = self.get_or_create_provider(model)?;
        provider
            .complete(model, messages, system, tools, token)
            .await
    }
}
