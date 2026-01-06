use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "result_type", rename_all = "snake_case")]
pub enum CompactResult {
    Success(String),
    Cancelled,
    InsufficientMessages,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "response_type", rename_all = "snake_case")]
pub enum CommandResponse {
    Text(String),
    Compact(CompactResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command_type", rename_all = "snake_case")]
pub enum AppCommandType {
    Model { target: Option<String> },
    Clear,
    Compact,
}

impl AppCommandType {
    pub fn parse(input: &str) -> Result<Self, SlashCommandError> {
        let command = input.trim();
        let command = command.strip_prefix('/').unwrap_or(command);

        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Err(SlashCommandError::InvalidFormat(
                "Empty command".to_string(),
            ));
        }

        match parts[0] {
            "model" => {
                let target = if parts.len() > 1 {
                    Some(parts[1..].join(" "))
                } else {
                    None
                };
                Ok(AppCommandType::Model { target })
            }
            "clear" => Ok(AppCommandType::Clear),
            "compact" => Ok(AppCommandType::Compact),
            cmd => Err(SlashCommandError::UnknownCommand(cmd.to_string())),
        }
    }

    pub fn as_command_str(&self) -> String {
        match self {
            AppCommandType::Model { target } => {
                if let Some(model) = target {
                    format!("model {model}")
                } else {
                    "model".to_string()
                }
            }
            AppCommandType::Clear => "clear".to_string(),
            AppCommandType::Compact => "compact".to_string(),
        }
    }
}

impl fmt::Display for AppCommandType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.as_command_str())
    }
}

impl FromStr for AppCommandType {
    type Err = SlashCommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SlashCommandError {
    #[error("Unknown command: {0}")]
    UnknownCommand(String),
    #[error("Invalid command format: {0}")]
    InvalidFormat(String),
}
