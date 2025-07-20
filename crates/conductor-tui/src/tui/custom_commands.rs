use crate::tui::commands::{CoreCommandType, TuiCommandType};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use strum::IntoEnumIterator;
use thiserror::Error;
use tracing::{debug, warn};

/// Errors that can occur with custom commands
#[derive(Debug, Error)]
pub enum CustomCommandError {
    #[error("Failed to load custom commands config: {0}")]
    ConfigLoadError(String),
    #[error("Failed to parse custom commands config: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Invalid command name '{0}': {1}")]
    InvalidCommandName(String, String),
    #[error("Command name '{0}' conflicts with built-in command")]
    ConflictingCommandName(String),
}

/// Represents a custom command that can be dynamically loaded
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CustomCommand {
    /// A simple prompt command that sends predefined text
    Prompt {
        name: String,
        description: String,
        prompt: String,
    },
    // Future command types can be added here:
    // Shell { name: String, description: String, command: String },
    // Macro { name: String, description: String, steps: Vec<String> },
}

impl CustomCommand {
    /// Get the name of the command
    pub fn name(&self) -> &str {
        match self {
            CustomCommand::Prompt { name, .. } => name,
        }
    }

    /// Get the description of the command
    pub fn description(&self) -> &str {
        match self {
            CustomCommand::Prompt { description, .. } => description,
        }
    }

    /// Validate the command configuration
    pub fn validate(&self) -> Result<(), CustomCommandError> {
        let name = self.name();

        // Check for empty name
        if name.is_empty() {
            return Err(CustomCommandError::InvalidCommandName(
                name.to_string(),
                "Command name cannot be empty".to_string(),
            ));
        }

        // Check for invalid characters
        if name.contains('/') || name.contains(' ') {
            return Err(CustomCommandError::InvalidCommandName(
                name.to_string(),
                "Command name cannot contain '/' or spaces".to_string(),
            ));
        }

        // Check for conflicts with built-in commands
        for cmd in TuiCommandType::iter() {
            if cmd.command_name() == name {
                return Err(CustomCommandError::ConflictingCommandName(name.to_string()));
            }
        }

        for cmd in CoreCommandType::iter() {
            if cmd.command_name() == name {
                return Err(CustomCommandError::ConflictingCommandName(name.to_string()));
            }
        }

        Ok(())
    }
}

/// Configuration file structure for custom commands
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CustomCommandsConfig {
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
}

/// Get all paths where custom commands can be defined, in order of precedence
pub fn get_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Project-specific config (highest precedence)
    paths.push(PathBuf::from(".conductor").join("commands.toml"));

    // 2. User config directory (platform-specific)
    if let Some(proj_dirs) = ProjectDirs::from("", "", "conductor") {
        paths.push(proj_dirs.config_dir().join("commands.toml"));
    }

    paths
}

/// Get the path to the custom commands configuration file
pub fn get_config_path() -> PathBuf {
    // For backwards compatibility, return the first writable path
    get_config_paths()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from(".conductor").join("commands.toml"))
}

/// Load custom commands from the configuration file
pub fn load_custom_commands() -> Result<Vec<CustomCommand>, CustomCommandError> {
    let mut all_commands = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Load from all paths in order of precedence
    for config_path in get_config_paths() {
        debug!("Checking for custom commands at: {}", config_path.display());

        if !config_path.exists() {
            debug!("Config not found at {}", config_path.display());
            continue;
        }

        let config_content = match std::fs::read_to_string(&config_path) {
            Ok(content) => content,
            Err(e) => {
                warn!("Failed to read {}: {}", config_path.display(), e);
                continue;
            }
        };

        let config: CustomCommandsConfig = match toml::from_str(&config_content) {
            Ok(config) => config,
            Err(e) => {
                warn!(
                    "Failed to parse {}: {}. Skipping this config file.",
                    config_path.display(),
                    e
                );
                continue;
            }
        };

        debug!(
            "Found {} commands in {}",
            config.commands.len(),
            config_path.display()
        );

        // Check for duplicates within the same file
        let mut file_names = std::collections::HashSet::new();
        for cmd in &config.commands {
            if !file_names.insert(cmd.name().to_string()) {
                warn!(
                    "Duplicate command '{}' found within {}. Skipping duplicates.",
                    cmd.name(),
                    config_path.display()
                );
            }
        }

        // Add commands, skipping duplicates (earlier paths take precedence)
        for cmd in config.commands {
            // Validate the command
            match cmd.validate() {
                Ok(()) => {
                    if seen_names.insert(cmd.name().to_string()) {
                        all_commands.push(cmd);
                    } else {
                        debug!(
                            "Skipping duplicate command '{}' from {}",
                            cmd.name(),
                            config_path.display()
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        "Skipping invalid command from {}: {}",
                        config_path.display(),
                        e
                    );
                }
            }
        }
    }

    debug!("Total custom commands loaded: {}", all_commands.len());
    Ok(all_commands)
}

/// Save custom commands to the configuration file
pub fn save_custom_commands(commands: &[CustomCommand]) -> Result<(), CustomCommandError> {
    let config_path = get_config_path();

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let config = CustomCommandsConfig {
        commands: commands.to_vec(),
    };

    let config_content = toml::to_string_pretty(&config).map_err(|e| {
        CustomCommandError::ConfigLoadError(format!("Failed to serialize config: {e}"))
    })?;

    std::fs::write(&config_path, config_content)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prompt_command() {
        let config_content = r#"
[[commands]]
type = "prompt"
name = "standup"
description = "Generate a standup report"
prompt = "What did I work on today? Check git log and recent file changes."
"#;

        let config: CustomCommandsConfig = toml::from_str(config_content).unwrap();
        assert_eq!(config.commands.len(), 1);

        match &config.commands[0] {
            CustomCommand::Prompt {
                name,
                description,
                prompt,
            } => {
                assert_eq!(name, "standup");
                assert_eq!(description, "Generate a standup report");
                assert_eq!(
                    prompt,
                    "What did I work on today? Check git log and recent file changes."
                );
            }
        }
    }

    #[test]
    fn test_multiple_commands() {
        let config_content = r#"
[[commands]]
type = "prompt"
name = "test"
description = "Run tests"
prompt = "Run the test suite and show me any failures"

[[commands]]
type = "prompt"
name = "review"
description = "Code review helper"
prompt = "Review the recent changes and suggest improvements"
"#;

        let config: CustomCommandsConfig = toml::from_str(config_content).unwrap();
        assert_eq!(config.commands.len(), 2);
        assert_eq!(config.commands[0].name(), "test");
        assert_eq!(config.commands[1].name(), "review");
    }
}
