pub mod claude;
pub mod error;
pub mod gemini;
pub mod messages;
pub mod openai;
pub mod provider;
pub mod tools;

use anyhow::{Result, anyhow};
use clap::builder::PossibleValue;
pub use claude::AnthropicClient;
pub use error::ApiError;
pub use gemini::GeminiClient;
pub use messages::ContentBlock;
pub use messages::Message;
use once_cell::sync::Lazy;
pub use openai::OpenAIClient;
pub use provider::{CompletionResponse, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use strum::Display;
use strum::IntoStaticStr;
use strum::{EnumIter, IntoEnumIterator};
use strum_macros::{AsRefStr, EnumString};
use tokio_util::sync::CancellationToken;
pub use tools::{InputSchema, Tool, ToolCall};
use tracing::warn;

use crate::config::LlmConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    Google,
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, AsRefStr, Display, IntoStaticStr,
)]
pub enum Model {
    #[strum(serialize = "claude-3-5-sonnet-20240620")]
    Claude3_5Sonnet20240620,
    #[strum(serialize = "claude-3-5-sonnet-20241022")]
    Claude3_5Sonnet20241022,
    #[strum(serialize = "claude-3-7-sonnet-20250219")]
    Claude3_7Sonnet20250219,
    #[strum(serialize = "claude-3-5-haiku-20241022")]
    Claude3_5Haiku20241022,
    #[strum(serialize = "claude-sonnet-4-20250514")]
    ClaudeSonnet4_20250514,
    #[strum(serialize = "claude-opus-4-20250514")]
    ClaudeOpus4_20250514,
    #[strum(serialize = "gpt-4.1-2025-04-14")]
    Gpt4_1_20250414,
    #[strum(serialize = "gpt-4.1-mini-2025-04-14")]
    Gpt4_1Mini20250414,
    #[strum(serialize = "gpt-4.1-nano-2025-04-14")]
    Gpt4_1Nano20250414,
    #[strum(serialize = "o3-2025-04-16")]
    O3_20250416,
    #[strum(serialize = "gemini-2.5-flash-preview-04-17")]
    Gemini2_5FlashPreview0417,
    #[strum(serialize = "gemini-2.5-pro-preview-05-06")]
    Gemini2_5ProPreview0506,
    #[strum(serialize = "gemini-2.5-pro-preview-06-05")]
    Gemini2_5ProPreview0605,
}

impl Model {
    pub fn provider(&self) -> ProviderKind {
        match self {
            Model::Claude3_7Sonnet20250219
            | Model::Claude3_5Sonnet20240620
            | Model::Claude3_5Sonnet20241022
            | Model::Claude3_5Haiku20241022
            | Model::ClaudeSonnet4_20250514
            | Model::ClaudeOpus4_20250514 => ProviderKind::Anthropic,

            Model::Gpt4_1_20250414
            | Model::Gpt4_1Mini20250414
            | Model::Gpt4_1Nano20250414
            | Model::O3_20250416 => ProviderKind::OpenAI,

            Model::Gemini2_5FlashPreview0417
            | Model::Gemini2_5ProPreview0506
            | Model::Gemini2_5ProPreview0605 => {
                ProviderKind::Google
            }
        }
    }
}

static MODEL_VARIANTS: Lazy<Vec<Model>> = Lazy::new(|| Model::iter().collect());

impl clap::ValueEnum for Model {
    fn value_variants<'a>() -> &'a [Self] {
        MODEL_VARIANTS.as_slice()
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let s: &'static str = (*self).into();
        Some(PossibleValue::new(s))
    }
}

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
            ProviderKind::Google => Arc::new(GeminiClient::new(key)),
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
    ) -> Result<CompletionResponse, ApiError> {
        let provider = self
            .get_or_create_provider(model)
            .map_err(|e| ApiError::Configuration(e.to_string()))?;

        if token.is_cancelled() {
            return Err(ApiError::Cancelled {
                provider: provider.name().to_string(),
            });
        }

        provider
            .complete(model, messages, system, tools, token)
            .await
    }

    pub async fn complete_with_retry(
        &self,
        model: Model,
        messages: &Vec<Message>,
        system_prompt: &Option<String>,
        tools: &Option<Vec<Tool>>,
        token: CancellationToken,
        max_attempts: usize,
    ) -> Result<CompletionResponse, ApiError> {
        let mut attempts = 0;

        loop {
            match self
                .complete(
                    model,
                    messages.clone(),
                    system_prompt.clone(),
                    tools.clone(),
                    token.clone(),
                )
                .await
            {
                Ok(response) => {
                    return Ok(response);
                }
                Err(error) => {
                    attempts += 1;
                    warn!(
                        "API completion attempt {}/{} failed for model {}: {:?}",
                        attempts,
                        max_attempts,
                        model.as_ref(),
                        error
                    );

                    if attempts >= max_attempts {
                        return Err(error);
                    }

                    match error {
                        ApiError::RateLimited { provider, details } => {
                            let sleep_duration =
                                std::time::Duration::from_secs(1 << (attempts - 1));
                            warn!(
                                "Rate limited by API: {} {} (retrying in {} seconds)",
                                provider,
                                details,
                                sleep_duration.as_secs()
                            );
                            tokio::time::sleep(sleep_duration).await;
                        }
                        ApiError::NoChoices { provider } => {
                            warn!("No choices returned from API: {}", provider);
                        }
                        ApiError::ServerError {
                            provider,
                            status_code,
                            details,
                        } => {
                            warn!(
                                "Server error for API: {} {} {}",
                                provider, status_code, details
                            );
                        }
                        _ => {
                            // Not retryable
                            return Err(error);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_model_from_str() {
        let model = Model::from_str("claude-3-7-sonnet-20250219").unwrap();
        assert_eq!(model, Model::Claude3_7Sonnet20250219);
    }
}
