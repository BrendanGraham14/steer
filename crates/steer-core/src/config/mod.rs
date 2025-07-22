use crate::api::ProviderKind;
use crate::auth::AuthStorage;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub model: Option<String>,
    pub history_size: Option<usize>,
    pub system_prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<NotificationSettings>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NotificationSettings {
    pub enable_sound: Option<bool>,
    pub enable_desktop: Option<bool>,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enable_sound: Some(true),
            enable_desktop: Some(true),
        }
    }
}

impl Config {
    fn new() -> Self {
        Self {
            model: Some("claude-3-7-sonnet-20250219".to_string()),
            history_size: Some(10),
            system_prompt: None,
            notifications: Some(NotificationSettings::default()),
        }
    }
}

/// Get the path to the config file
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| Error::Configuration("Could not find config directory".to_string()))?
        .join("steer");

    fs::create_dir_all(&config_dir)
        .map_err(|e| Error::Configuration(format!("Failed to create config directory: {e}")))?;

    Ok(config_dir.join("config.json"))
}

/// Load the configuration
pub fn load_config() -> Result<Config> {
    let config_path = get_config_path()?;

    if !config_path.exists() {
        return Ok(Config::new());
    }

    let config_str = fs::read_to_string(&config_path)
        .map_err(|e| Error::Configuration(format!("Failed to read config file: {e}")))?;

    let config: Config = serde_json::from_str(&config_str)
        .map_err(|e| Error::Configuration(format!("Failed to parse config file: {e}")))?;

    Ok(config)
}

/// Initialize or update the configuration
pub fn init_config(force: bool) -> Result<()> {
    let config_path = get_config_path()?;

    if config_path.exists() && !force {
        return Err(Error::Configuration(
            "Config file already exists. Use --force to overwrite.".to_string(),
        ));
    }

    let config = Config::new();
    let config_json = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Configuration(format!("Failed to serialize config: {e}")))?;

    fs::write(&config_path, config_json)
        .map_err(|e| Error::Configuration(format!("Failed to write config file: {e}")))?;

    Ok(())
}

/// Save the configuration
pub fn save_config(config: &Config) -> Result<()> {
    let config_path = get_config_path()?;
    let config_json = serde_json::to_string_pretty(&config)
        .map_err(|e| Error::Configuration(format!("Failed to serialize config: {e}")))?;

    fs::write(&config_path, config_json)
        .map_err(|e| Error::Configuration(format!("Failed to write config file: {e}")))?;

    Ok(())
}

#[derive(Debug, Clone)]
pub enum ApiAuth {
    Key(String),
    OAuth,
}

/// Provider for authentication credentials
#[derive(Clone)]
pub struct LlmConfigProvider {
    storage: Arc<dyn AuthStorage>,
}

impl std::fmt::Debug for LlmConfigProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfigProvider").finish_non_exhaustive()
    }
}

impl LlmConfigProvider {
    /// Create a new LlmConfigProvider with the given auth storage
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self { storage }
    }

    /// Get authentication for a specific provider
    pub async fn get_auth_for_provider(&self, provider: ProviderKind) -> Result<Option<ApiAuth>> {
        // For Anthropic: Check OAuth tokens first, then env vars, then stored API key
        match provider {
            ProviderKind::Anthropic => {
                // API key via env var > OAuth > stored API key
                let anthropic_key = std::env::var("CLAUDE_API_KEY")
                    .ok()
                    .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());
                if let Some(key) = anthropic_key {
                    Ok(Some(ApiAuth::Key(key)))
                } else if self
                    .storage
                    .get_credential("anthropic", crate::auth::CredentialType::AuthTokens)
                    .await?
                    .is_some()
                {
                    Ok(Some(ApiAuth::OAuth))
                } else {
                    {
                        // Fall back to stored API key in keyring
                        if let Some(crate::auth::Credential::ApiKey { value }) = self
                            .storage
                            .get_credential("anthropic", crate::auth::CredentialType::ApiKey)
                            .await?
                        {
                            Ok(Some(ApiAuth::Key(value)))
                        } else {
                            Ok(None)
                        }
                    }
                }
            }
            ProviderKind::OpenAI => {
                // API key via env var > stored API key
                if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                    Ok(Some(ApiAuth::Key(key)))
                } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                    .storage
                    .get_credential("openai", crate::auth::CredentialType::ApiKey)
                    .await?
                {
                    Ok(Some(ApiAuth::Key(value)))
                } else {
                    Ok(None)
                }
            }
            ProviderKind::Google => {
                // API key via env var > stored API key
                if let Ok(key) = std::env::var("GEMINI_API_KEY") {
                    Ok(Some(ApiAuth::Key(key)))
                } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                    .storage
                    .get_credential("google", crate::auth::CredentialType::ApiKey)
                    .await?
                {
                    Ok(Some(ApiAuth::Key(value)))
                } else {
                    Ok(None)
                }
            }
            ProviderKind::XAI => {
                // API key via env var > stored API key
                if let Ok(key) = std::env::var("XAI_API_KEY") {
                    Ok(Some(ApiAuth::Key(key)))
                } else if let Ok(key) = std::env::var("GROK_API_KEY") {
                    Ok(Some(ApiAuth::Key(key)))
                } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                    .storage
                    .get_credential("xai", crate::auth::CredentialType::ApiKey)
                    .await?
                {
                    Ok(Some(ApiAuth::Key(value)))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Get the auth storage
    pub fn auth_storage(&self) -> &Arc<dyn AuthStorage> {
        &self.storage
    }

    /// Return list of providers that have authentication configured
    pub async fn available_providers(&self) -> Result<Vec<ProviderKind>> {
        let mut providers = Vec::new();
        if self
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await?
            .is_some()
        {
            providers.push(ProviderKind::Anthropic);
        }
        if self
            .get_auth_for_provider(ProviderKind::OpenAI)
            .await?
            .is_some()
        {
            providers.push(ProviderKind::OpenAI);
        }
        if self
            .get_auth_for_provider(ProviderKind::Google)
            .await?
            .is_some()
        {
            providers.push(ProviderKind::Google);
        }
        if self
            .get_auth_for_provider(ProviderKind::XAI)
            .await?
            .is_some()
        {
            providers.push(ProviderKind::XAI);
        }
        Ok(providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::InMemoryAuthStorage;

    #[tokio::test]
    async fn test_auth_changes_immediately_reflected() {
        // Create a provider with in-memory storage
        let storage = Arc::new(InMemoryAuthStorage::new());
        let provider = LlmConfigProvider::new(storage.clone());

        // Initially no auth
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(auth.is_none());

        // Add API key
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::ApiKey {
                    value: "test-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately see the new key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::Key(key)) if key == "test-key"));

        // Add OAuth tokens
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::AuthTokens(crate::auth::storage::AuthTokens {
                    access_token: "access".to_string(),
                    refresh_token: "refresh".to_string(),
                    expires_at: std::time::SystemTime::now() + std::time::Duration::from_secs(3600),
                }),
            )
            .await
            .unwrap();

        // Should immediately prefer OAuth over API key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::OAuth)));

        // Remove OAuth tokens
        storage
            .remove_credential("anthropic", crate::auth::CredentialType::AuthTokens)
            .await
            .unwrap();

        // Should immediately fall back to API key
        let auth = provider
            .get_auth_for_provider(ProviderKind::Anthropic)
            .await
            .unwrap();
        assert!(matches!(auth, Some(ApiAuth::Key(key)) if key == "test-key"));
    }

    #[tokio::test]
    async fn test_available_providers_updates_immediately() {
        let storage = Arc::new(InMemoryAuthStorage::new());
        let provider = LlmConfigProvider::new(storage.clone());

        // Initially no providers
        let providers = provider.available_providers().await.unwrap();
        assert!(providers.is_empty());

        // Add Anthropic API key
        storage
            .set_credential(
                "anthropic",
                crate::auth::Credential::ApiKey {
                    value: "test-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately show Anthropic
        let providers = provider.available_providers().await.unwrap();
        assert_eq!(providers, vec![ProviderKind::Anthropic]);

        // Add OpenAI key
        storage
            .set_credential(
                "openai",
                crate::auth::Credential::ApiKey {
                    value: "openai-key".to_string(),
                },
            )
            .await
            .unwrap();

        // Should immediately show both
        let providers = provider.available_providers().await.unwrap();
        assert_eq!(
            providers,
            vec![ProviderKind::Anthropic, ProviderKind::OpenAI]
        );
    }
}
