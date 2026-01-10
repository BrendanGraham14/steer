use crate::auth::AuthStorage;
use crate::config::provider::ProviderId;
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
            model: Some(crate::config::model::builtin::opus().1),
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
    env_provider: Arc<dyn EnvProvider>,
}

impl std::fmt::Debug for LlmConfigProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfigProvider").finish_non_exhaustive()
    }
}

impl LlmConfigProvider {
    /// Create a new LlmConfigProvider with the given auth storage
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            storage,
            env_provider: Arc::new(StdEnvProvider),
        }
    }

    /// Create a new LlmConfigProvider with a custom env provider (useful for tests).
    #[cfg(test)]
    fn with_env_provider(
        storage: Arc<dyn AuthStorage>,
        env_provider: Arc<dyn EnvProvider>,
    ) -> Self {
        Self {
            storage,
            env_provider,
        }
    }

    /// Get authentication for a specific provider ID
    pub async fn get_auth_for_provider(&self, provider_id: &ProviderId) -> Result<Option<ApiAuth>> {
        if provider_id.as_str() == self::provider::ANTHROPIC_ID {
            // API key via env var > OAuth > stored API key
            let anthropic_key = self
                .env_provider
                .var("CLAUDE_API_KEY")
                .or_else(|| self.env_provider.var("ANTHROPIC_API_KEY"));
            if let Some(key) = anthropic_key {
                Ok(Some(ApiAuth::Key(key)))
            } else if self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::OAuth2,
                )
                .await?
                .is_some()
            {
                Ok(Some(ApiAuth::OAuth))
            } else {
                // Fall back to stored API key in keyring
                if let Some(crate::auth::Credential::ApiKey { value }) = self
                    .storage
                    .get_credential(
                        &provider_id.storage_key(),
                        crate::auth::CredentialType::ApiKey,
                    )
                    .await?
                {
                    Ok(Some(ApiAuth::Key(value)))
                } else {
                    Ok(None)
                }
            }
        } else if provider_id.as_str() == self::provider::OPENAI_ID {
            // OAuth > env API key > stored API key
            if self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::OAuth2,
                )
                .await?
                .is_some()
            {
                Ok(Some(ApiAuth::OAuth))
            } else if let Some(key) = self.env_provider.var("OPENAI_API_KEY") {
                Ok(Some(ApiAuth::Key(key)))
            } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::ApiKey,
                )
                .await?
            {
                Ok(Some(ApiAuth::Key(value)))
            } else {
                Ok(None)
            }
        } else if provider_id.as_str() == self::provider::GOOGLE_ID {
            // API key via env var > stored API key
            if let Some(key) = self
                .env_provider
                .var("GEMINI_API_KEY")
                .or_else(|| self.env_provider.var("GOOGLE_API_KEY"))
            {
                Ok(Some(ApiAuth::Key(key)))
            } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::ApiKey,
                )
                .await?
            {
                Ok(Some(ApiAuth::Key(value)))
            } else {
                Ok(None)
            }
        } else if provider_id.as_str() == self::provider::XAI_ID {
            // API key via env var > stored API key
            if let Some(key) = self
                .env_provider
                .var("XAI_API_KEY")
                .or_else(|| self.env_provider.var("GROK_API_KEY"))
            {
                Ok(Some(ApiAuth::Key(key)))
            } else if let Some(crate::auth::Credential::ApiKey { value }) = self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::ApiKey,
                )
                .await?
            {
                Ok(Some(ApiAuth::Key(value)))
            } else {
                Ok(None)
            }
        } else {
            // Custom providers - check for stored API key
            if let Some(crate::auth::Credential::ApiKey { value }) = self
                .storage
                .get_credential(
                    &provider_id.storage_key(),
                    crate::auth::CredentialType::ApiKey,
                )
                .await?
            {
                Ok(Some(ApiAuth::Key(value)))
            } else {
                Ok(None)
            }
        }
    }

    /// Resolve authentication for a provider using server-side auto-selection.
    ///
    /// This is a compatibility shim; it currently delegates to `get_auth_for_provider`.
    pub async fn resolve_auth_for_provider(
        &self,
        provider_id: &ProviderId,
    ) -> Result<Option<ApiAuth>> {
        self.get_auth_for_provider(provider_id).await
    }

    /// Get the auth storage
    pub fn auth_storage(&self) -> &Arc<dyn AuthStorage> {
        &self.storage
    }
}

pub mod model;
pub mod provider;
pub mod toml_types;

trait EnvProvider: Send + Sync {
    fn var(&self, key: &str) -> Option<String>;
}

#[derive(Clone)]
struct StdEnvProvider;

impl EnvProvider for StdEnvProvider {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthTokens;
    use crate::test_utils::InMemoryAuthStorage;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    #[derive(Clone, Default)]
    struct TestEnvProvider {
        vars: HashMap<String, String>,
    }

    impl EnvProvider for TestEnvProvider {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }

    #[tokio::test]
    async fn openai_oauth_takes_precedence() {
        let storage = Arc::new(InMemoryAuthStorage::new());
        storage
            .set_credential(
                "openai",
                crate::auth::Credential::ApiKey {
                    value: "stored-key".to_string(),
                },
            )
            .await
            .unwrap();
        storage
            .set_credential(
                "openai",
                crate::auth::Credential::OAuth2(AuthTokens {
                    access_token: "token".to_string(),
                    refresh_token: "refresh".to_string(),
                    expires_at: SystemTime::now() + Duration::from_secs(3600),
                    id_token: None,
                }),
            )
            .await
            .unwrap();

        let mut env = TestEnvProvider::default();
        env.vars
            .insert("OPENAI_API_KEY".to_string(), "env-key".to_string());
        let provider = LlmConfigProvider::with_env_provider(storage, Arc::new(env));
        let auth = provider
            .get_auth_for_provider(&provider::openai())
            .await
            .unwrap();

        assert!(matches!(auth, Some(ApiAuth::OAuth)));
    }

    #[tokio::test]
    async fn openai_env_takes_precedence_over_stored_key() {
        let storage = Arc::new(InMemoryAuthStorage::new());
        storage
            .set_credential(
                "openai",
                crate::auth::Credential::ApiKey {
                    value: "stored-key".to_string(),
                },
            )
            .await
            .unwrap();

        let mut env = TestEnvProvider::default();
        env.vars
            .insert("OPENAI_API_KEY".to_string(), "env-key".to_string());
        let provider = LlmConfigProvider::with_env_provider(storage, Arc::new(env));
        let auth = provider
            .get_auth_for_provider(&provider::openai())
            .await
            .unwrap();

        match auth {
            Some(ApiAuth::Key(key)) => assert_eq!(key, "env-key"),
            _ => panic!("Expected env API key"),
        }
    }
}
