//! Style constants for TUI widgets
use ratatui::style::{Color, Modifier, Style};

// Role styles
pub const ROLE_USER: Style = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
pub const ROLE_ASSISTANT: Style = Style::new().fg(Color::Blue).add_modifier(Modifier::BOLD);
pub const ROLE_SYSTEM: Style = Style::new().fg(Color::Yellow);

// Tool styles
pub const TOOL_BOX: Style = Style::new().fg(Color::Cyan);
pub const TOOL_HEADER: Style = Style::new().fg(Color::Cyan);
pub const TOOL_ID: Style = Style::new().fg(Color::DarkGray);
pub const TOOL_SUCCESS: Style = Style::new().fg(Color::Green);
pub const TOOL_ERROR: Style = Style::new().fg(Color::Red);

// Message styles
pub const THOUGHT_BOX: Style = Style::new().fg(Color::DarkGray);
pub const THOUGHT_HEADER: Style = Style::new().fg(Color::Gray);
pub const THOUGHT_TEXT: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);

// Command execution styles
pub const COMMAND_PROMPT: Style = Style::new().fg(Color::Green).add_modifier(Modifier::BOLD);
pub const COMMAND_TEXT: Style = Style::new().fg(Color::Cyan);
pub const COMMAND_SUCCESS_BOX: Style = Style::new().fg(Color::Green);
pub const COMMAND_ERROR_BOX: Style = Style::new().fg(Color::Red);

// Selection/highlight styles
pub const SELECTION_HIGHLIGHT: Style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);

// Text styles
pub const ITALIC_GRAY: Style = Style::new()
    .fg(Color::DarkGray)
    .add_modifier(Modifier::ITALIC);
pub const DIM_TEXT: Style = Style::new().fg(Color::DarkGray);
pub const ERROR_TEXT: Style = Style::new().fg(Color::Red);
pub const ERROR_BOLD: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);

// Border styles
pub const INPUT_BORDER_NORMAL: Style = Style::new().fg(Color::DarkGray);
pub const INPUT_BORDER_ACTIVE: Style = Style::new().fg(Color::Yellow);
pub const INPUT_BORDER_COMMAND: Style = Style::new().fg(Color::Cyan);
pub const INPUT_BORDER_APPROVAL: Style = Style::new().fg(Color::Red).add_modifier(Modifier::BOLD);
pub const INPUT_BORDER_EXIT: Style = Style::new()
    .fg(Color::LightRed)
    .add_modifier(Modifier::BOLD);

// Model info style
pub const MODEL_INFO: Style = Style::new().fg(Color::LightMagenta);
