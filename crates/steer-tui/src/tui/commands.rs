pub mod registry;

use crate::tui::core_commands::{CoreCommandType as CoreCommand, SlashCommandError};
use crate::tui::custom_commands::CustomCommand;
use std::fmt;
use std::str::FromStr;
use strum::{Display, EnumIter, IntoEnumIterator};
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
#[derive(Debug, Clone, PartialEq)]
pub enum TuiCommand {
    /// Start a new conversation session
    New,
    /// Reload files in the TUI
    ReloadFiles,
    /// Change or list themes
    Theme(Option<String>),
    /// Launch authentication setup
    Auth,
    /// Show help for commands
    Help(Option<String>),
    /// Switch editing mode
    EditingMode(Option<String>),
    /// Show MCP server connection status
    Mcp,
    /// Show workspace status
    Workspace(Option<String>),
    /// Custom user-defined command
    Custom(CustomCommand),
}

/// Enum representing all TUI command types (without parameters)
/// This is used for exhaustive iteration and type-safe handling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display)]
#[strum(serialize_all = "kebab-case")]
pub enum TuiCommandType {
    New,
    ReloadFiles,
    Theme,
    Auth,
    Help,
    EditingMode,
    Mcp,
    Workspace,
}

impl TuiCommandType {
    pub fn command_name(&self) -> String {
        match self {
            TuiCommandType::New => self.to_string(),
            TuiCommandType::ReloadFiles => self.to_string(),
            TuiCommandType::Theme => self.to_string(),
            TuiCommandType::Auth => self.to_string(),
            TuiCommandType::Help => self.to_string(),
            TuiCommandType::EditingMode => self.to_string(),
            TuiCommandType::Mcp => self.to_string(),
            TuiCommandType::Workspace => self.to_string(),
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            TuiCommandType::New => "Start a new conversation session",
            TuiCommandType::ReloadFiles => "Reload file cache in the TUI",
            TuiCommandType::Theme => "Change or list available themes",
            TuiCommandType::Auth => "Manage authentication settings",
            TuiCommandType::Help => "Show help information",
            TuiCommandType::EditingMode => "Switch between editing modes (simple/vim)",
            TuiCommandType::Mcp => "Show MCP server connection status",
            TuiCommandType::Workspace => "Show workspace status",
        }
    }

    pub fn usage(&self) -> String {
        match self {
            TuiCommandType::New => format!("/{}", self.command_name()),
            TuiCommandType::ReloadFiles => format!("/{}", self.command_name()),
            TuiCommandType::Theme => format!("/{} [theme_name]", self.command_name()),
            TuiCommandType::Auth => format!("/{}", self.command_name()),
            TuiCommandType::Help => format!("/{} [command]", self.command_name()),
            TuiCommandType::EditingMode => format!("/{} [simple|vim]", self.command_name()),
            TuiCommandType::Mcp => format!("/{}", self.command_name()),
            TuiCommandType::Workspace => format!("/{} [workspace_id]", self.command_name()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, Display)]
#[strum(serialize_all = "kebab-case")]
pub enum CoreCommandType {
    Model,
    Agent,
    Compact,
}

impl CoreCommandType {
    pub fn command_name(&self) -> String {
        match self {
            CoreCommandType::Model => self.to_string(),
            CoreCommandType::Agent => self.to_string(),
            CoreCommandType::Compact => self.to_string(),
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            CoreCommandType::Model => "Show or change the current model",
            CoreCommandType::Agent => "Switch primary agent mode (normal/planner/yolo)",
            CoreCommandType::Compact => "Summarize the current conversation",
        }
    }

    pub fn usage(&self) -> String {
        match self {
            CoreCommandType::Model => format!("/{} [model_name]", self.command_name()),
            CoreCommandType::Agent => format!("/{} <mode>", self.command_name()),
            CoreCommandType::Compact => format!("/{}", self.command_name()),
        }
    }

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
            CoreCommandType::Agent => {
                let target = if args.is_empty() {
                    None
                } else {
                    Some(args.join(" "))
                };
                Some(CoreCommand::Agent { target })
            }
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

        for cmd_type in TuiCommandType::iter() {
            if cmd_name == cmd_type.command_name() {
                return match cmd_type {
                    TuiCommandType::New => Ok(TuiCommand::New),
                    TuiCommandType::ReloadFiles => Ok(TuiCommand::ReloadFiles),
                    TuiCommandType::Theme => {
                        let theme_name = parts.get(1).map(|s| (*s).to_string());
                        Ok(TuiCommand::Theme(theme_name))
                    }
                    TuiCommandType::Auth => Ok(TuiCommand::Auth),
                    TuiCommandType::Help => {
                        let command_name = parts.get(1).map(|s| (*s).to_string());
                        Ok(TuiCommand::Help(command_name))
                    }
                    TuiCommandType::EditingMode => {
                        let mode_name = parts.get(1).map(|s| (*s).to_string());
                        Ok(TuiCommand::EditingMode(mode_name))
                    }
                    TuiCommandType::Mcp => Ok(TuiCommand::Mcp),
                    TuiCommandType::Workspace => {
                        let workspace_id = parts.get(1).map(|s| (*s).to_string());
                        Ok(TuiCommand::Workspace(workspace_id))
                    }
                };
            }
        }

        Err(TuiCommandError::UnknownCommand(command.to_string()))
    }

    pub fn as_command_str(&self) -> String {
        match self {
            TuiCommand::New => TuiCommandType::New.command_name().to_string(),
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
            TuiCommand::EditingMode(None) => TuiCommandType::EditingMode.command_name().to_string(),
            TuiCommand::EditingMode(Some(mode)) => {
                format!("{} {}", TuiCommandType::EditingMode.command_name(), mode)
            }
            TuiCommand::Mcp => TuiCommandType::Mcp.command_name().to_string(),
            TuiCommand::Workspace(None) => TuiCommandType::Workspace.command_name().to_string(),
            TuiCommand::Workspace(Some(workspace_id)) => {
                format!(
                    "{} {}",
                    TuiCommandType::Workspace.command_name(),
                    workspace_id
                )
            }
            TuiCommand::Custom(cmd) => cmd.name().to_string(),
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
                }
                return Err(TuiCommandError::UnknownCommand(command.to_string()));
            }
        }

        if cmd_name == "mode" {
            let args: Vec<&str> = parts.into_iter().skip(1).collect();
            let target = if args.is_empty() {
                None
            } else {
                Some(args.join(" "))
            };
            return Ok(AppCommand::Core(CoreCommand::Agent { target }));
        }

        // Note: Custom commands will be resolved by the caller using the registry
        // since we can't access the registry from here
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
        assert!(matches!(
            AppCommand::parse("/mcp").unwrap(),
            AppCommand::Tui(TuiCommand::Mcp)
        ));
        assert!(matches!(
            AppCommand::parse("/workspace").unwrap(),
            AppCommand::Tui(TuiCommand::Workspace(None))
        ));
    }

    #[test]
    fn test_parse_core_commands() {
        assert!(matches!(
            AppCommand::parse("/help").unwrap(),
            AppCommand::Tui(TuiCommand::Help(None))
        ));
        assert!(matches!(
            AppCommand::parse("/model opus").unwrap(),
            AppCommand::Core(CoreCommand::Model { .. })
        ));
        assert!(matches!(
            AppCommand::parse("/compact").unwrap(),
            AppCommand::Core(CoreCommand::Compact)
        ));
        assert!(matches!(
            AppCommand::parse("/agent planner").unwrap(),
            AppCommand::Core(CoreCommand::Agent { .. })
        ));
        assert!(matches!(
            AppCommand::parse("/mode yolo").unwrap(),
            AppCommand::Core(CoreCommand::Agent { .. })
        ));
    }

    #[test]
    fn test_parse_new_command() {
        assert!(matches!(
            AppCommand::parse("/new").unwrap(),
            AppCommand::Tui(TuiCommand::New)
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
