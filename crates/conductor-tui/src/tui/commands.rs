pub mod registry;

use conductor_core::app::conversation::{AppCommandType as CoreCommand, SlashCommandError};
use std::fmt;
use std::str::FromStr;
use strum::{Display, EnumIter, EnumString, IntoEnumIterator};
use thiserror::Error;

/// Errors that can occur when parsing TUI commands
#[derive(Debug, Error)]
pub enum TuiCommandError {
    #[error("Unknown command: {0}")]
    UnknownCommand(String),
    #[error(transparent)]
    CoreParseError(#[from] SlashCommandError),
}

/// TUI-specific commands that don't belong in the core
#[derive(Debug, Clone, PartialEq, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum TuiCommand {
    /// Reload files in the TUI
    ReloadFiles,
    /// Change or list themes
    Theme(Option<String>),
    /// Launch authentication setup
    Auth,
    /// Show help for commands
    Help(Option<String>),
}

/// Enum representing all TUI command types (without parameters)
/// This is used for exhaustive iteration and type-safe handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display)]
#[strum(serialize_all = "kebab-case")]
pub enum TuiCommandType {
    ReloadFiles,
    Theme,
    Auth,
    Help,
}

impl TuiCommandType {
    /// Get the command name as it appears in slash commands
    pub fn command_name(&self) -> String {
        match self {
            TuiCommandType::ReloadFiles => self.to_string(),
            TuiCommandType::Theme => self.to_string(),
            TuiCommandType::Auth => self.to_string(),
            TuiCommandType::Help => self.to_string(),
        }
    }

    /// Get the command description
    pub fn description(&self) -> &'static str {
        match self {
            TuiCommandType::ReloadFiles => "Reload file cache in the TUI",
            TuiCommandType::Theme => "Change or list available themes",
            TuiCommandType::Auth => "Manage authentication settings",
            TuiCommandType::Help => "Show help information",
        }
    }

    /// Get the command usage
    pub fn usage(&self) -> String {
        match self {
            TuiCommandType::ReloadFiles => format!("/{}", self.command_name()),
            TuiCommandType::Theme => format!("/{} [theme_name]", self.command_name()),
            TuiCommandType::Auth => format!("/{}", self.command_name()),
            TuiCommandType::Help => format!("/{} [command]", self.command_name()),
        }
    }
}

/// Enum representing all Core command types (without parameters)
/// This mirrors conductor_core::app::conversation::AppCommandType
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display)]
#[strum(serialize_all = "kebab-case")]
pub enum CoreCommandType {
    Model,
    Clear,
    Compact,
}

impl CoreCommandType {
    /// Get the command name as it appears in slash commands
    pub fn command_name(&self) -> String {
        match self {
            CoreCommandType::Model => self.to_string(),
            CoreCommandType::Clear => self.to_string(),
            CoreCommandType::Compact => self.to_string(),
        }
    }

    /// Get the command description
    pub fn description(&self) -> &'static str {
        match self {
            CoreCommandType::Model => "Show or change the current model",
            CoreCommandType::Clear => "Clear conversation history and tool approvals",
            CoreCommandType::Compact => "Summarise older messages to save context space",
        }
    }

    /// Get the command usage
    pub fn usage(&self) -> String {
        match self {
            CoreCommandType::Model => format!("/{} [model_name]", self.command_name()),
            CoreCommandType::Clear => format!("/{}", self.command_name()),
            CoreCommandType::Compact => format!("/{}", self.command_name()),
        }
    }

    /// Convert to the actual core AppCommandType
    /// Returns None if the command requires parameters that aren't provided
    pub fn to_core_command(&self, args: &[&str]) -> Option<CoreCommand> {
        match self {
            CoreCommandType::Model => {
                let target = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                Some(CoreCommand::Model { target })
            }
            CoreCommandType::Clear => Some(CoreCommand::Clear),
            CoreCommandType::Compact => Some(CoreCommand::Compact),
        }
    }
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
        let cmd_name = parts.first().copied().unwrap_or("");

        // Try to match against all TuiCommandType variants
        for cmd_type in TuiCommandType::iter() {
            if cmd_name == cmd_type.command_name() {
                return match cmd_type {
                    TuiCommandType::ReloadFiles => Ok(TuiCommand::ReloadFiles),
                    TuiCommandType::Theme => {
                        let theme_name = parts.get(1).map(|s| s.to_string());
                        Ok(TuiCommand::Theme(theme_name))
                    }
                    TuiCommandType::Auth => Ok(TuiCommand::Auth),
                    TuiCommandType::Help => {
                        let command_name = parts.get(1).map(|s| s.to_string());
                        Ok(TuiCommand::Help(command_name))
                    }
                };
            }
        }

        Err(TuiCommandError::UnknownCommand(command.to_string()))
    }

    /// Convert the command to its string representation (without leading slash)
    pub fn as_command_str(&self) -> String {
        match self {
            TuiCommand::ReloadFiles => TuiCommandType::ReloadFiles.command_name().to_string(),
            TuiCommand::Theme(None) => TuiCommandType::Theme.command_name().to_string(),
            TuiCommand::Theme(Some(name)) => {
                format!("{} {}", TuiCommandType::Theme.command_name(), name)
            }
            TuiCommand::Auth => TuiCommandType::Auth.command_name().to_string(),
            TuiCommand::Help(None) => TuiCommandType::Help.command_name().to_string(),
            TuiCommand::Help(Some(cmd)) => {
                format!("{} {}", TuiCommandType::Help.command_name(), cmd)
            }
        }
    }
}

impl AppCommand {
    /// Parse a command string into an AppCommand
    pub fn parse(input: &str) -> Result<Self, TuiCommandError> {
        // Trim whitespace and remove leading slash if present
        let command = input.trim();
        let command = command.strip_prefix('/').unwrap_or(command);

        let parts: Vec<&str> = command.split_whitespace().collect();
        let cmd_name = parts.first().copied().unwrap_or("");

        // First try to parse as a TUI command
        for tui_type in TuiCommandType::iter() {
            if cmd_name == tui_type.command_name() {
                return TuiCommand::parse_without_slash(command).map(AppCommand::Tui);
            }
        }

        // Then try to parse as a Core command
        for core_type in CoreCommandType::iter() {
            if cmd_name == core_type.command_name() {
                let args: Vec<&str> = parts.into_iter().skip(1).collect();
                if let Some(core_cmd) = core_type.to_core_command(&args) {
                    return Ok(AppCommand::Core(core_cmd));
                } else {
                    return Err(TuiCommandError::UnknownCommand(command.to_string()));
                }
            }
        }

        Err(TuiCommandError::UnknownCommand(command.to_string()))
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
        assert!(matches!(
            AppCommand::parse("/reload-files").unwrap(),
            AppCommand::Tui(TuiCommand::ReloadFiles)
        ));
        assert!(matches!(
            AppCommand::parse("/theme").unwrap(),
            AppCommand::Tui(TuiCommand::Theme(None))
        ));
        assert!(matches!(
            AppCommand::parse("/theme gruvbox").unwrap(),
            AppCommand::Tui(TuiCommand::Theme(Some(_)))
        ));
    }

    #[test]
    fn test_parse_core_commands() {
        assert!(matches!(
            AppCommand::parse("/help").unwrap(),
            AppCommand::Tui(TuiCommand::Help(None))
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
        assert_eq!(AppCommand::Tui(TuiCommand::Help(None)).to_string(), "/help");
    }

    #[test]
    fn test_error_formatting() {
        // Test TUI unknown command error
        let err = AppCommand::parse("/unknown-tui-cmd").unwrap_err();
        assert_eq!(err.to_string(), "Unknown command: unknown-tui-cmd");
    }

    #[test]
    fn test_tui_command_from_str() {
        let cmd = "/reload-files".parse::<AppCommand>().unwrap();
        assert!(matches!(cmd, AppCommand::Tui(TuiCommand::ReloadFiles)));
    }
}
