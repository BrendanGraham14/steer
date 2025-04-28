use crate::api::ProviderKind;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub history_size: Option<usize>,
    pub openai_api_key: Option<String>,
}

impl Config {
    fn new() -> Self {
        Self {
            api_key: None,
            model: Some("claude-3-7-sonnet-20250219".to_string()),
            history_size: Some(10),
            openai_api_key: None,
        }
    }

    /// Get the API key for the specified provider
    pub fn get_api_key_for_provider(&self, provider: &str) -> Option<String> {
        match provider {
            "openai" => self.openai_api_key.clone().or_else(|| self.api_key.clone()),
            "claude" | _ => self.api_key.clone(),
        }
    }
}

/// Get the path to the config file
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Could not find config directory")?
        .join("claude-code-rs");

    fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

    Ok(config_dir.join("config.json"))
}

/// Load the configuration
pub fn load_config() -> Result<Config> {
    let config_path = get_config_path()?;

    if !config_path.exists() {
        return Ok(Config::new());
    }

    let config_str = fs::read_to_string(&config_path).context("Failed to read config file")?;

    let config: Config =
        serde_json::from_str(&config_str).context("Failed to parse config file")?;

    Ok(config)
}

/// Initialize or update the configuration
pub fn init_config(force: bool) -> Result<()> {
    let config_path = get_config_path()?;

    if config_path.exists() && !force {
        return Err(anyhow::anyhow!(
            "Config file already exists. Use --force to overwrite."
        ));
    }

    let config = Config::new();
    let config_json =
        serde_json::to_string_pretty(&config).context("Failed to serialize config")?;

    fs::write(&config_path, config_json).context("Failed to write config file")?;

    Ok(())
}

/// Save the configuration
pub fn save_config(config: &Config) -> Result<()> {
    let config_path = get_config_path()?;
    let config_json =
        serde_json::to_string_pretty(&config).context("Failed to serialize config")?;

    fs::write(&config_path, config_json).context("Failed to write config file")?;

    Ok(())
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub anthropic_key: Option<String>,
    pub openai_key: Option<String>,
}

impl LlmConfig {
    pub fn from_env() -> anyhow::Result<Self> {
        let cfg = Self {
            anthropic_key: std::env::var("CLAUDE_API_KEY")
                .ok()
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok()),
            openai_key: std::env::var("OPENAI_API_KEY").ok(),
        };
        Ok(cfg)
    }

    pub fn key_for(&self, provider: ProviderKind) -> Option<&str> {
        match provider {
            ProviderKind::Anthropic => self.anthropic_key.as_deref(),
            ProviderKind::OpenAI => self.openai_key.as_deref(),
        }
    }

    /// Return list of providers that have an API key configured
    pub fn available_providers(&self) -> Vec<ProviderKind> {
        let mut providers = Vec::new();
        if self.anthropic_key.is_some() {
            providers.push(ProviderKind::Anthropic);
        }
        if self.openai_key.is_some() {
            providers.push(ProviderKind::OpenAI);
        }
        providers
    }
}
