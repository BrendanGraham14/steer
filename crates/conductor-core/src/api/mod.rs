pub mod claude;
pub mod error;
pub mod gemini;
pub mod grok;
pub mod openai;
pub mod provider;

use crate::config::{ApiAuth, LlmConfig};
use crate::error::Result;
pub use claude::AnthropicClient;
pub use conductor_tools::{InputSchema, ToolCall, ToolSchema};
pub use error::ApiError;
pub use gemini::GeminiClient;
pub use grok::GrokClient;
pub use openai::OpenAIClient;
pub use provider::{CompletionResponse, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use strum::Display;
use strum::EnumIter;
use strum::IntoStaticStr;
use strum_macros::{AsRefStr, EnumString};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::app::conversation::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, IntoStaticStr)]
#[strum(serialize_all = "lowercase")]
pub enum ProviderKind {
    Anthropic,
    OpenAI,
    Google,
    Grok,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    EnumIter,
    EnumString,
    AsRefStr,
    Display,
    IntoStaticStr,
    serde::Serialize,
    serde::Deserialize,
    Default,
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
    #[strum(serialize = "claude-sonnet-4-20250514", serialize = "sonnet")]
    ClaudeSonnet4_20250514,
    #[strum(serialize = "claude-opus-4-20250514", serialize = "opus")]
    #[default]
    ClaudeOpus4_20250514,
    #[strum(serialize = "gpt-4.1-2025-04-14")]
    Gpt4_1_20250414,
    #[strum(serialize = "gpt-4.1-mini-2025-04-14")]
    Gpt4_1Mini20250414,
    #[strum(serialize = "gpt-4.1-nano-2025-04-14")]
    Gpt4_1Nano20250414,
    #[strum(serialize = "o3-2025-04-16", serialize = "o3")]
    O3_20250416,
    #[strum(serialize = "o3-pro-2025-06-10", serialize = "o3-pro")]
    O3Pro20250610,
    #[strum(serialize = "o4-mini-2025-04-16", serialize = "o4-mini")]
    O4Mini20250416,
    #[strum(serialize = "gemini-2.5-flash-preview-04-17")]
    Gemini2_5FlashPreview0417,
    #[strum(serialize = "gemini-2.5-pro-preview-05-06")]
    Gemini2_5ProPreview0506,
    #[strum(serialize = "gemini-2.5-pro-preview-06-05", serialize = "gemini")]
    Gemini2_5ProPreview0605,
    #[strum(serialize = "grok-3", serialize = "grok")]
    Grok3,
    #[strum(serialize = "grok-3-mini", serialize = "grok-mini")]
    Grok3Mini,
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
            | Model::O3_20250416
            | Model::O3Pro20250610
            | Model::O4Mini20250416 => ProviderKind::OpenAI,

            Model::Gemini2_5FlashPreview0417
            | Model::Gemini2_5ProPreview0506
            | Model::Gemini2_5ProPreview0605 => ProviderKind::Google,

            Model::Grok3 | Model::Grok3Mini => ProviderKind::Grok,
        }
    }

    pub fn aliases(&self) -> Vec<&'static str> {
        match self {
            Model::ClaudeSonnet4_20250514 => vec!["sonnet"],
            Model::ClaudeOpus4_20250514 => vec!["opus"],
            Model::O3_20250416 => vec!["o3"],
            Model::O3Pro20250610 => vec!["o3-pro"],
            Model::O4Mini20250416 => vec!["o4-mini"],
            Model::Gemini2_5ProPreview0605 => vec!["gemini"],
            Model::Grok3 => vec!["grok"],
            Model::Grok3Mini => vec!["grok-mini"],
            _ => vec![],
        }
    }

    pub fn supports_thinking(&self) -> bool {
        matches!(
            self,
            Model::Claude3_7Sonnet20250219
                | Model::ClaudeSonnet4_20250514
                | Model::ClaudeOpus4_20250514
                | Model::O3_20250416
                | Model::O3Pro20250610
                | Model::O4Mini20250416
                | Model::Gemini2_5FlashPreview0417
                | Model::Gemini2_5ProPreview0506
                | Model::Gemini2_5ProPreview0605
                | Model::Grok3Mini
        )
    }

    pub fn default_system_prompt_file(&self) -> Option<&'static str> {
        match self {
            Model::O3_20250416 => Some("models/o3.md"),
            Model::O3Pro20250610 => Some("models/o3.md"),
            Model::O4Mini20250416 => Some("models/o3.md"),
            _ => None,
        }
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
        let provider_instance: Arc<dyn Provider> = match self.config.auth_for(provider_kind) {
            Some(ApiAuth::OAuth) => {
                if provider_kind == ProviderKind::Anthropic {
                    let storage = self.config.auth_storage().ok_or_else(|| {
                        crate::error::Error::Api(ApiError::Configuration(
                            "OAuth configured but no auth storage available".to_string(),
                        ))
                    })?;
                    Arc::new(AnthropicClient::with_oauth(storage.clone()))
                } else {
                    return Err(crate::error::Error::Api(ApiError::Configuration(format!(
                        "OAuth is not supported for {provider_kind:?} provider"
                    ))));
                }
            }
            Some(ApiAuth::Key(key)) => match provider_kind {
                ProviderKind::Anthropic => Arc::new(AnthropicClient::with_api_key(&key)),
                ProviderKind::OpenAI => Arc::new(OpenAIClient::new(key)),
                ProviderKind::Google => Arc::new(GeminiClient::new(&key)),
                ProviderKind::Grok => Arc::new(GrokClient::new(key)),
            },

            None => {
                return Err(crate::error::Error::Api(ApiError::Configuration(format!(
                    "No authentication configured for {provider_kind:?} needed by model {model:?}"
                ))));
            }
        };
        map.insert(model, provider_instance.clone());
        Ok(provider_instance)
    }

    pub async fn complete(
        &self,
        model: Model,
        messages: Vec<Message>,
        system: Option<String>,
        tools: Option<Vec<ToolSchema>>,
        token: CancellationToken,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        let provider = self.get_or_create_provider(model).map_err(ApiError::from)?;

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
        messages: &[Message],
        system_prompt: &Option<String>,
        tools: &Option<Vec<ToolSchema>>,
        token: CancellationToken,
        max_attempts: usize,
    ) -> std::result::Result<CompletionResponse, ApiError> {
        let mut attempts = 0;
        debug!(target: "api::complete", "model: {:?} messages: {:?}", model, messages);
        loop {
            match self
                .complete(
                    model,
                    messages.to_vec(),
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

    #[test]
    fn test_model_aliases() {
        // Test short aliases
        assert_eq!(
            Model::from_str("sonnet").unwrap(),
            Model::ClaudeSonnet4_20250514
        );
        assert_eq!(
            Model::from_str("opus").unwrap(),
            Model::ClaudeOpus4_20250514
        );
        assert_eq!(Model::from_str("o3").unwrap(), Model::O3_20250416);
        assert_eq!(Model::from_str("o3-pro").unwrap(), Model::O3Pro20250610);
        assert_eq!(
            Model::from_str("gemini").unwrap(),
            Model::Gemini2_5ProPreview0605
        );
        assert_eq!(Model::from_str("grok").unwrap(), Model::Grok3);
        assert_eq!(Model::from_str("grok-mini").unwrap(), Model::Grok3Mini);

        // Also test the full names work
        assert_eq!(
            Model::from_str("claude-sonnet-4-20250514").unwrap(),
            Model::ClaudeSonnet4_20250514
        );
        assert_eq!(
            Model::from_str("o3-2025-04-16").unwrap(),
            Model::O3_20250416
        );

        assert_eq!(
            Model::from_str("o4-mini-2025-04-16").unwrap(),
            Model::O4Mini20250416
        );
        assert_eq!(Model::from_str("grok-3").unwrap(), Model::Grok3);
        assert_eq!(Model::from_str("grok").unwrap(), Model::Grok3);
        assert_eq!(Model::from_str("grok-3-mini").unwrap(), Model::Grok3Mini);
        assert_eq!(Model::from_str("grok-mini").unwrap(), Model::Grok3Mini);
    }
}
