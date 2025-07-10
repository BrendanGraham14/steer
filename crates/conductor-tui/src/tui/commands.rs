use conductor_core::app::conversation::{AppCommandType as CoreCommand, SlashCommandError};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Errors that can occur when parsing TUI commands
#[derive(Debug, Error)]
pub enum TuiCommandError {
    UnknownCommand(String),
    CoreParseError(#[from] SlashCommandError),
}

// Custom Display implementation to provide consistent error messages
impl fmt::Display for TuiCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TuiCommandError::UnknownCommand(cmd) => write!(f, "Unknown command: {cmd}"),
            TuiCommandError::CoreParseError(core_err) => {
                // Provide consistent formatting for all error types
                match core_err {
                    SlashCommandError::UnknownCommand(cmd) => write!(f, "Unknown command: {cmd}"),
                    SlashCommandError::InvalidFormat(msg) => {
                        write!(f, "Invalid command format: {msg}")
                    }
                }
            }
        }
    }
}

/// TUI-specific commands that don't belong in the core
#[derive(Debug, Clone, PartialEq)]
pub enum TuiCommand {
    /// Reload files in the TUI
    ReloadFiles,
    /// Change or list themes
    Theme(Option<String>),
    /// Launch authentication setup
    Auth,
}

/// Unified command type that can represent either TUI or Core commands
#[derive(Debug, Clone, PartialEq)]
pub enum AppCommand {
    /// A TUI-specific command
    Tui(TuiCommand),
    /// A core command that gets passed down
    Core(CoreCommand),
}

impl TuiCommand {
    /// Parse a command string into a TuiCommand (without leading slash)
    fn parse_without_slash(command: &str) -> Result<Self, TuiCommandError> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        match parts.first() {
            Some(&"reload-files") => Ok(TuiCommand::ReloadFiles),
            Some(&"theme") => {
                let theme_name = parts.get(1).map(|s| s.to_string());
                Ok(TuiCommand::Theme(theme_name))
            }
            Some(&"auth") => Ok(TuiCommand::Auth),
            _ => Err(TuiCommandError::UnknownCommand(command.to_string())),
        }
    }

    /// Convert the command to its string representation (without leading slash)
    pub fn as_command_str(&self) -> String {
        match self {
            TuiCommand::ReloadFiles => "reload-files".to_string(),
            TuiCommand::Theme(None) => "theme".to_string(),
            TuiCommand::Theme(Some(name)) => format!("theme {name}"),
            TuiCommand::Auth => "auth".to_string(),
        }
    }
}

impl AppCommand {
    /// Parse a command string into an AppCommand
    pub fn parse(input: &str) -> Result<Self, TuiCommandError> {
        // Trim whitespace and remove leading slash if present
        let command = input.trim();
        let command = command.strip_prefix('/').unwrap_or(command);

        // First try to parse as a TUI command
        match TuiCommand::parse_without_slash(command) {
            Ok(tui_cmd) => Ok(AppCommand::Tui(tui_cmd)),
            Err(_) => {
                // If not a TUI command, try to parse as a core command
                CoreCommand::parse(input)
                    .map(AppCommand::Core)
                    .map_err(TuiCommandError::from)
            }
        }
    }

    /// Convert the command back to its string representation (with leading slash)
    pub fn as_command_str(&self) -> String {
        match self {
            AppCommand::Tui(tui_cmd) => format!("/{}", tui_cmd.as_command_str()),
            AppCommand::Core(core_cmd) => core_cmd.to_string(),
        }
    }
}

impl fmt::Display for TuiCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.as_command_str())
    }
}

impl fmt::Display for AppCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_command_str())
    }
}

impl FromStr for AppCommand {
    type Err = TuiCommandError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tui_commands() {
        assert_eq!(
            AppCommand::parse("/reload-files").unwrap(),
            AppCommand::Tui(TuiCommand::ReloadFiles)
        );
        assert_eq!(
            AppCommand::parse("reload-files").unwrap(),
            AppCommand::Tui(TuiCommand::ReloadFiles)
        );
    }

    #[test]
    fn test_parse_core_commands() {
        assert!(matches!(
            AppCommand::parse("/help").unwrap(),
            AppCommand::Core(CoreCommand::Help)
        ));
        assert!(matches!(
            AppCommand::parse("/clear").unwrap(),
            AppCommand::Core(CoreCommand::Clear)
        ));
        assert!(matches!(
            AppCommand::parse("/model opus").unwrap(),
            AppCommand::Core(CoreCommand::Model { .. })
        ));
    }

    #[test]
    fn test_display() {
        assert_eq!(
            AppCommand::Tui(TuiCommand::ReloadFiles).to_string(),
            "/reload-files"
        );
        assert_eq!(AppCommand::Core(CoreCommand::Help).to_string(), "/help");
    }

    #[test]
    fn test_error_formatting() {
        // Test TUI unknown command error
        let err = AppCommand::parse("/unknown-tui-cmd").unwrap_err();
        assert_eq!(err.to_string(), "Unknown command: unknown-tui-cmd");

        // Test core unknown command error (will be caught by core parser)
        let err = AppCommand::parse("/unknown-core-cmd").unwrap_err();
        assert_eq!(err.to_string(), "Unknown command: unknown-core-cmd");
    }
}
