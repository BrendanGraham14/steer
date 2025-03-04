use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Config {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub history_size: Option<usize>,
}

impl Config {
    fn new() -> Self {
        Self {
            api_key: None,
            model: Some("claude-3-7-sonnet-20250219".to_string()),
            history_size: Some(10),
        }
    }
}

/// Get the path to the config file
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Could not find config directory")?
        .join("claude-code-rs");
    
    fs::create_dir_all(&config_dir)
        .context("Failed to create config directory")?;
    
    Ok(config_dir.join("config.json"))
}

/// Load the configuration
pub fn load_config() -> Result<Config> {
    let config_path = get_config_path()?;
    
    if !config_path.exists() {
        return Ok(Config::new());
    }
    
    let config_str = fs::read_to_string(&config_path)
        .context("Failed to read config file")?;
    
    let config: Config = serde_json::from_str(&config_str)
        .context("Failed to parse config file")?;
    
    Ok(config)
}

/// Initialize or update the configuration
pub fn init_config(force: bool) -> Result<()> {
    let config_path = get_config_path()?;
    
    if config_path.exists() && !force {
        return Err(anyhow::anyhow!("Config file already exists. Use --force to overwrite."));
    }
    
    let config = Config::new();
    let config_json = serde_json::to_string_pretty(&config)
        .context("Failed to serialize config")?;
    
    fs::write(&config_path, config_json)
        .context("Failed to write config file")?;
    
    Ok(())
}

/// Save the configuration
pub fn save_config(config: &Config) -> Result<()> {
    let config_path = get_config_path()?;
    let config_json = serde_json::to_string_pretty(&config)
        .context("Failed to serialize config")?;
    
    fs::write(&config_path, config_json)
        .context("Failed to write config file")?;
    
    Ok(())
}