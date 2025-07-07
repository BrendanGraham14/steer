use crate::api::ProviderKind;
use crate::auth::AuthStorage;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

const CACHE_DURATION: u64 = 600;

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
        .join("conductor");

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

#[derive(Clone, Default)]
pub struct LlmConfig {
    pub anthropic_auth: Option<ApiAuth>,
    pub openai_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub auth_storage: Option<Arc<dyn AuthStorage>>,
}

impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfig")
            .field("anthropic_auth", &self.anthropic_auth)
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "gemini_api_key",
                &self.gemini_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("auth_storage", &self.auth_storage.is_some())
            .finish()
    }
}

impl LlmConfig {
    /// Create a new builder for LlmConfig
    pub fn builder() -> LlmConfigBuilder {
        LlmConfigBuilder::default()
    }

    pub fn auth_for(&self, provider: ProviderKind) -> Option<ApiAuth> {
        match provider {
            ProviderKind::Anthropic => self.anthropic_auth.clone(),
            ProviderKind::OpenAI => self
                .openai_api_key
                .as_ref()
                .map(|k| ApiAuth::Key(k.clone())),
            ProviderKind::Google => self
                .gemini_api_key
                .as_ref()
                .map(|k| ApiAuth::Key(k.clone())),
        }
    }

    pub fn auth_storage(&self) -> Option<&Arc<dyn AuthStorage>> {
        self.auth_storage.as_ref()
    }

    /// Return list of providers that have authentication configured
    pub fn available_providers(&self) -> Vec<ProviderKind> {
        let mut providers = Vec::new();
        if self.anthropic_auth.is_some() {
            providers.push(ProviderKind::Anthropic);
        }
        if self.openai_api_key.is_some() {
            providers.push(ProviderKind::OpenAI);
        }
        if self.gemini_api_key.is_some() {
            providers.push(ProviderKind::Google);
        }
        providers
    }
}

/// Builder for LlmConfig
#[derive(Default)]
pub struct LlmConfigBuilder {
    anthropic_auth: Option<ApiAuth>,
    openai_api_key: Option<String>,
    gemini_api_key: Option<String>,
    auth_storage: Option<Arc<dyn AuthStorage>>,
}

impl LlmConfigBuilder {
    pub fn with_anthropic_auth(mut self, auth: ApiAuth) -> Self {
        self.anthropic_auth = Some(auth);
        self
    }

    pub fn with_openai_api_key(mut self, key: String) -> Self {
        self.openai_api_key = Some(key);
        self
    }

    pub fn with_gemini_api_key(mut self, key: String) -> Self {
        self.gemini_api_key = Some(key);
        self
    }

    pub fn with_auth_storage(mut self, storage: Arc<dyn AuthStorage>) -> Self {
        self.auth_storage = Some(storage);
        self
    }

    pub fn build(self) -> LlmConfig {
        LlmConfig {
            anthropic_auth: self.anthropic_auth,
            openai_api_key: self.openai_api_key,
            gemini_api_key: self.gemini_api_key,
            auth_storage: self.auth_storage,
        }
    }
}

/// Loader for LlmConfig that handles async operations
pub struct LlmConfigLoader {
    storage: Arc<dyn AuthStorage>,
}

impl LlmConfigLoader {
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self { storage }
    }

    pub async fn from_env(&self) -> Result<LlmConfig> {
        // Check for OAuth tokens first (takes precedence)
        let anthropic_auth = if self.storage.get_tokens("anthropic").await?.is_some() {
            Some(ApiAuth::OAuth)
        } else {
            // If no OAuth, check for API key
            let anthropic_key = std::env::var("CLAUDE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());
            anthropic_key.map(ApiAuth::Key)
        };

        Ok(LlmConfig {
            anthropic_auth,
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            gemini_api_key: std::env::var("GEMINI_API_KEY").ok(),
            auth_storage: Some(self.storage.clone()),
        })
    }

    /// Load configuration from environment, returning empty config if no credentials are found
    /// This is useful for tests that don't require actual API access
    pub async fn from_env_allow_missing(&self) -> LlmConfig {
        self.from_env().await.unwrap_or_else(|_| LlmConfig {
            anthropic_auth: None,
            openai_api_key: None,
            gemini_api_key: None,
            auth_storage: Some(self.storage.clone()),
        })
    }
}

/// Provider for LlmConfig with caching and thread-safe access
#[derive(Clone)]
pub struct LlmConfigProvider {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LlmConfigProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmConfigProvider").finish_non_exhaustive()
    }
}

struct Inner {
    storage: Arc<dyn AuthStorage>,
    cache: tokio::sync::RwLock<Option<(std::time::SystemTime, LlmConfig)>>,
}

impl LlmConfigProvider {
    /// Create a new LlmConfigProvider with the given auth storage
    pub fn new(storage: Arc<dyn AuthStorage>) -> Self {
        Self {
            inner: Arc::new(Inner {
                storage,
                cache: tokio::sync::RwLock::new(None),
            }),
        }
    }

    /// Get the current LlmConfig, using cache if valid
    pub async fn get(&self) -> Result<LlmConfig> {
        // Check cache first
        {
            let cache = self.inner.cache.read().await;
            if let Some((timestamp, config)) = cache.as_ref() {
                let elapsed = std::time::SystemTime::now()
                    .duration_since(*timestamp)
                    .unwrap_or(std::time::Duration::MAX);

                // Cache is valid for 10 minutes
                if elapsed.as_secs() < CACHE_DURATION {
                    return Ok(config.clone());
                }
            }
        }

        // Cache miss or expired, load fresh config
        let loader = LlmConfigLoader::new(self.inner.storage.clone());
        let config = loader.from_env().await?;

        // Update cache
        {
            let mut cache = self.inner.cache.write().await;
            *cache = Some((std::time::SystemTime::now(), config.clone()));
        }

        Ok(config)
    }

    /// Get the current LlmConfig, returning empty config if no credentials are found
    /// This is useful for tests that don't require actual API access
    pub async fn get_allow_missing(&self) -> LlmConfig {
        self.get().await.unwrap_or_else(|_| LlmConfig {
            anthropic_auth: None,
            openai_api_key: None,
            gemini_api_key: None,
            auth_storage: Some(self.inner.storage.clone()),
        })
    }
}
