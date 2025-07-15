pub mod claude;
pub mod error;
pub mod factory;
pub mod gemini;
pub mod openai;
pub mod provider;
pub mod util;
pub mod xai;

use crate::auth::ProviderRegistry;
use crate::auth::storage::{Credential, CredentialType};
use crate::config::{ApiAuth, LlmConfigProvider};
use crate::error::Result;
pub use error::ApiError;
pub use factory::{create_provider, create_provider_with_storage};
pub use provider::{CompletionResponse, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
pub use steer_tools::{InputSchema, ToolCall, ToolSchema};
use strum::Display;
use strum::EnumIter;
use strum::IntoStaticStr;
use strum_macros::{AsRefStr, EnumString};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use tracing::warn;

use crate::app::conversation::Message;

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
    #[strum(serialize = "claude-opus-4-20250514", serialize = "opus-4")]
    ClaudeOpus4_20250514,
    #[strum(
        serialize = "claude-opus-4-1-20250805",
        serialize = "opus",
        serialize = "opus-4-1"
    )]
    #[default]
    ClaudeOpus4_1_20250805,
    #[strum(serialize = "gpt-4.1-2025-04-14")]
    Gpt4_1_20250414,
    #[strum(serialize = "gpt-4.1-mini-2025-04-14")]
    Gpt4_1Mini20250414,
    #[strum(serialize = "gpt-4.1-nano-2025-04-14")]
    Gpt4_1Nano20250414,
    #[strum(serialize = "gpt-5-2025-08-07", serialize = "gpt-5")]
    Gpt5_20250807,
    #[strum(serialize = "o3-2025-04-16", serialize = "o3")]
    O3_20250416,
    #[strum(serialize = "o3-pro-2025-06-10", serialize = "o3-pro")]
    O3Pro20250610,
    #[strum(serialize = "o4-mini-2025-04-16", serialize = "o4-mini")]
    O4Mini20250416,
    #[strum(serialize = "codex-mini-latest", serialize = "codex-mini")]
    CodexMiniLatest,
    #[strum(serialize = "gemini-2.5-flash-preview-04-17")]
    Gemini2_5FlashPreview0417,
    #[strum(serialize = "gemini-2.5-pro-preview-05-06")]
    Gemini2_5ProPreview0506,
    #[strum(serialize = "gemini-2.5-pro-preview-06-05", serialize = "gemini")]
    Gemini2_5ProPreview0605,
    #[strum(serialize = "grok-3")]
    Grok3,
    #[strum(serialize = "grok-3-mini", serialize = "grok-mini")]
    Grok3Mini,
    #[strum(serialize = "grok-4-0709", serialize = "grok")]
    Grok4_0709,
}

impl Model {
    /// Returns true if this model should be shown in the model picker UI
    pub fn should_show(&self) -> bool {
        matches!(
            self,
            Model::ClaudeOpus4_20250514
                | Model::ClaudeOpus4_1_20250805
                | Model::ClaudeSonnet4_20250514
                | Model::O3_20250416
                | Model::O3Pro20250610
                | Model::Gemini2_5ProPreview0605
                | Model::Grok4_0709
                | Model::Grok3
                | Model::Gpt4_1_20250414
                | Model::Gpt5_20250807
                | Model::O4Mini20250416
                | Model::CodexMiniLatest
        )
    }

    pub fn iter_recommended() -> impl Iterator<Item = Model> {
        use strum::IntoEnumIterator;
        Model::iter().filter(|m| m.should_show())
    }

    /// Returns the provider ID for this model.
    pub fn provider_id(&self) -> crate::config::provider::ProviderId {
        use crate::config::provider::ProviderId;
        match self {
            Model::Claude3_7Sonnet20250219
            | Model::Claude3_5Sonnet20240620
            | Model::Claude3_5Sonnet20241022
            | Model::Claude3_5Haiku20241022
            | Model::ClaudeSonnet4_20250514
            | Model::ClaudeOpus4_20250514
            | Model::ClaudeOpus4_1_20250805 => ProviderId::Anthropic,

            Model::Gpt4_1_20250414
            | Model::Gpt4_1Mini20250414
            | Model::Gpt4_1Nano20250414
            | Model::Gpt5_20250807
            | Model::O3_20250416
            | Model::O3Pro20250610
            | Model::O4Mini20250416
            | Model::CodexMiniLatest => ProviderId::Openai,

            Model::Gemini2_5FlashPreview0417
            | Model::Gemini2_5ProPreview0506
            | Model::Gemini2_5ProPreview0605 => ProviderId::Google,

            Model::Grok3 | Model::Grok3Mini | Model::Grok4_0709 => ProviderId::Xai,
        }
    }

    pub fn aliases(&self) -> Vec<&'static str> {
        match self {
            Model::ClaudeSonnet4_20250514 => vec!["sonnet"],
            Model::ClaudeOpus4_20250514 => vec!["opus-4-0"],
            Model::ClaudeOpus4_1_20250805 => vec!["opus-4-1", "opus"],
            Model::O3_20250416 => vec!["o3"],
            Model::O3Pro20250610 => vec!["o3-pro"],
            Model::O4Mini20250416 => vec!["o4-mini"],
            Model::Gemini2_5ProPreview0605 => vec!["gemini"],
            Model::Grok3 => vec![],
            Model::Grok3Mini => vec!["grok-mini"],
            Model::Grok4_0709 => vec!["grok"],
            Model::Gpt5_20250807 => vec!["gpt-5"],
            Model::CodexMiniLatest => vec!["codex-mini"],
            _ => vec![],
        }
    }

    pub fn supports_thinking(&self) -> bool {
        matches!(
            self,
            Model::Claude3_7Sonnet20250219
                | Model::ClaudeSonnet4_20250514
                | Model::ClaudeOpus4_20250514
                | Model::ClaudeOpus4_1_20250805
                | Model::Gpt5_20250807
                | Model::O3_20250416
                | Model::O3Pro20250610
                | Model::O4Mini20250416
                | Model::Gemini2_5FlashPreview0417
                | Model::Gemini2_5ProPreview0506
                | Model::Gemini2_5ProPreview0605
                | Model::Grok3Mini
                | Model::Grok4_0709
                | Model::CodexMiniLatest
        )
    }

    /// Get all available models
    pub fn all() -> Vec<Model> {
        use strum::IntoEnumIterator;
        Model::iter().collect()
    }
}

#[derive(Clone)]
pub struct Client {
    provider_map: Arc<RwLock<HashMap<Model, Arc<dyn Provider>>>>,
    config_provider: LlmConfigProvider,
    provider_registry: Arc<ProviderRegistry>,
}

impl Client {
    pub fn new_with_provider(provider: LlmConfigProvider) -> Self {
        // Load the provider registry
        let provider_registry =
            Arc::new(ProviderRegistry::load().expect("Failed to load provider registry"));

        Self {
            provider_map: Arc::new(RwLock::new(HashMap::new())),
            config_provider: provider,
            provider_registry,
        }
    }

    async fn get_or_create_provider(&self, model: Model) -> Result<Arc<dyn Provider>> {
        // First check without holding the lock across await
        {
            let map = self.provider_map.read().unwrap();
            if let Some(provider) = map.get(&model) {
                return Ok(provider.clone());
            }
        }

        // Get provider ID directly from the model
        let provider_id = model.provider_id();

        // Get the provider config from registry
        let provider_config = self.provider_registry.get(&provider_id).ok_or_else(|| {
            crate::error::Error::Api(ApiError::Configuration(format!(
                "No provider configuration found for {provider_id:?}"
            )))
        })?;

        // Get credential for the provider
        let credential = match self
            .config_provider
            .get_auth_for_provider(&provider_id)
            .await?
        {
            Some(ApiAuth::OAuth) => {
                // Get OAuth credential from storage using the centralized storage_key()
                self.config_provider
                    .auth_storage()
                    .get_credential(&provider_id.storage_key(), CredentialType::OAuth2)
                    .await
                    .map_err(|e| {
                        crate::error::Error::Api(ApiError::Configuration(format!(
                            "Failed to get OAuth credential: {e}"
                        )))
                    })?
                    .ok_or_else(|| {
                        crate::error::Error::Api(ApiError::Configuration(
                            "OAuth credential not found in storage".to_string(),
                        ))
                    })?
            }
            Some(ApiAuth::Key(key)) => Credential::ApiKey { value: key },
            None => {
                return Err(crate::error::Error::Api(ApiError::Configuration(format!(
                    "No authentication configured for {provider_id:?} needed by model {model:?}"
                ))));
            }
        };

        // Now acquire write lock and create provider
        let mut map = self.provider_map.write().unwrap();

        // Check again in case another thread added it
        if let Some(provider) = map.get(&model) {
            return Ok(provider.clone());
        }

        // Create the provider using factory
        let provider_instance = if matches!(&credential, Credential::OAuth2(_)) {
            factory::create_provider_with_storage(
                provider_config,
                &credential,
                self.config_provider.auth_storage().clone(),
            )
            .map_err(crate::error::Error::Api)?
        } else {
            factory::create_provider(provider_config, &credential)
                .map_err(crate::error::Error::Api)?
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
        let provider = self
            .get_or_create_provider(model)
            .await
            .map_err(ApiError::from)?;

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
        debug!(
            target: "api::complete",
            model =% model,
            "system: {:?}",
            system_prompt
        );
        debug!(
            target: "api::complete",
            model =% model,
            "messages: {:?}",
            messages
        );
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
            Model::ClaudeOpus4_1_20250805
        );
        assert_eq!(Model::from_str("o3").unwrap(), Model::O3_20250416);
        assert_eq!(Model::from_str("o3-pro").unwrap(), Model::O3Pro20250610);
        assert_eq!(
            Model::from_str("gemini").unwrap(),
            Model::Gemini2_5ProPreview0605
        );
        assert_eq!(Model::from_str("grok").unwrap(), Model::Grok4_0709);
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
        assert_eq!(Model::from_str("grok").unwrap(), Model::Grok4_0709);
        assert_eq!(Model::from_str("grok-4-0709").unwrap(), Model::Grok4_0709);
        assert_eq!(Model::from_str("grok-3-mini").unwrap(), Model::Grok3Mini);
        assert_eq!(Model::from_str("grok-mini").unwrap(), Model::Grok3Mini);
        assert_eq!(
            Model::from_str("gpt-5-2025-08-07").unwrap(),
            Model::Gpt5_20250807
        );
        assert_eq!(Model::from_str("gpt-5").unwrap(), Model::Gpt5_20250807);
    }
}
