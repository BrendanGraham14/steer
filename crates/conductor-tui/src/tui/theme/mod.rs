//! Theme system for conductor-tui
//!
//! This module provides a flexible theming system that allows users to customize
//! the appearance of the TUI without recompilation. Themes are loaded from TOML
//! files and can be switched at runtime.

use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::fmt;
use thiserror::Error;

mod loader;

pub use loader::ThemeLoader;

/// Errors that can occur during theme operations
#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Color not found in palette: {0}")]
    ColorNotFound(String),

    #[error("Invalid color value: {0}")]
    InvalidColor(String),
}

/// A color value that can be either a palette reference or a direct color
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ColorValue {
    /// Reference to a palette color (e.g., "background", "red")
    Palette(String),
    /// Direct color value (e.g., "#ff0000", "red")
    Direct(String),
}

/// Style definition for a component
#[derive(Debug, Clone, Deserialize)]
pub struct ComponentStyle {
    pub fg: Option<ColorValue>,
    pub bg: Option<ColorValue>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub underlined: bool,
}

/// Raw theme as loaded from TOML file
#[derive(Debug, Clone, Deserialize)]
pub struct RawTheme {
    pub name: String,
    pub palette: HashMap<String, RgbColor>,
    pub components: HashMap<Component, ComponentStyle>,
}

/// Theme is now an alias for CompiledTheme for easier use
pub type Theme = CompiledTheme;

/// RGB color that can be deserialized from hex strings
#[derive(Debug, Clone, Copy)]
pub struct RgbColor(pub u8, pub u8, pub u8);

impl<'de> Deserialize<'de> for RgbColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;

        // Try hex color first
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 {
                let r = u8::from_str_radix(&hex[0..2], 16)
                    .map_err(|_| serde::de::Error::custom(format!("Invalid hex color: {s}")))?;
                let g = u8::from_str_radix(&hex[2..4], 16)
                    .map_err(|_| serde::de::Error::custom(format!("Invalid hex color: {s}")))?;
                let b = u8::from_str_radix(&hex[4..6], 16)
                    .map_err(|_| serde::de::Error::custom(format!("Invalid hex color: {s}")))?;
                return Ok(RgbColor(r, g, b));
            }
        }

        // Try named colors
        match s.to_lowercase().as_str() {
            "black" => Ok(RgbColor(0, 0, 0)),
            "red" => Ok(RgbColor(255, 0, 0)),
            "green" => Ok(RgbColor(0, 255, 0)),
            "yellow" => Ok(RgbColor(255, 255, 0)),
            "blue" => Ok(RgbColor(0, 0, 255)),
            "magenta" => Ok(RgbColor(255, 0, 255)),
            "cyan" => Ok(RgbColor(0, 255, 255)),
            "white" => Ok(RgbColor(255, 255, 255)),
            "gray" | "grey" => Ok(RgbColor(128, 128, 128)),
            "darkgray" | "darkgrey" | "dark_gray" | "dark_grey" => Ok(RgbColor(64, 64, 64)),
            _ => Err(serde::de::Error::custom(format!("Unknown color: {s}"))),
        }
    }
}

impl From<RgbColor> for Color {
    fn from(rgb: RgbColor) -> Self {
        Color::Rgb(rgb.0, rgb.1, rgb.2)
    }
}

/// All themeable components in the TUI
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    // Status bar
    StatusBar,

    // Input panel
    InputPanelBorder,
    InputPanelBorderActive,
    InputPanelBorderCommand,
    InputPanelBorderApproval,
    InputPanelBorderError,
    InputPanelLabel,
    InputPanelLabelActive,
    InputPanelLabelCommand,
    InputPanelLabelConfirmExit,

    // Chat list
    ChatListBorder,
    UserMessage,
    UserMessageRole,
    AssistantMessage,
    AssistantMessageRole,
    SystemMessage,
    SystemMessageRole,

    // Tool calls
    ToolCall,
    ToolCallBorder,
    ToolCallHeader,
    ToolCallId,
    ToolOutput,
    ToolSuccess,
    ToolError,

    // Assistant thoughts
    ThoughtBox,
    ThoughtHeader,
    ThoughtBorder,
    ThoughtText,

    // Commands
    CommandPrompt,
    CommandText,
    CommandSuccess,
    CommandError,

    // General
    ErrorText,
    ErrorBold,
    DimText,
    SelectionHighlight,
    PlaceholderText,

    // Model info
    ModelInfo,

    // Notices
    NoticeInfo,
    NoticeWarn,
    NoticeError,

    // Todo items
    TodoHigh,
    TodoMedium,
    TodoLow,
    TodoPending,
    TodoInProgress,
    TodoCompleted,

    // Code editing
    CodeAddition,
    CodeDeletion,
    CodeFilePath,

    // Popup
    PopupBorder,
    PopupSelection,

    // Markdown elements
    MarkdownH1,
    MarkdownH2,
    MarkdownH3,
    MarkdownH4,
    MarkdownH5,
    MarkdownH6,
    MarkdownParagraph,
    MarkdownBold,
    MarkdownItalic,
    MarkdownStrikethrough,
    MarkdownCode,
    MarkdownCodeBlock,
    MarkdownLink,
    MarkdownBlockquote,
    MarkdownListBullet,
    MarkdownListNumber,

    // Markdown table elements
    MarkdownTableBorder,
    MarkdownTableHeader,
    MarkdownTableCell,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Compiled theme ready for use in the TUI
#[derive(Debug, Clone)]
pub struct CompiledTheme {
    pub name: String,
    pub styles: HashMap<Component, Style>,
    pub background_color: Option<Color>,
}

impl RawTheme {
    /// Compile the theme into a usable format
    pub fn into_theme(self) -> Result<Theme, ThemeError> {
        let mut styles = HashMap::new();

        // Extract background color from palette if it exists
        let background_color = self.palette.get("background").map(|&rgb| rgb.into());

        // Build the style for each component
        for (component, style_def) in &self.components {
            let mut style = Style::default();

            // Resolve foreground color
            if let Some(fg) = &style_def.fg {
                let color = self.resolve_color(fg.clone())?;
                style = style.fg(color);
            }

            // Resolve background color
            if let Some(bg) = &style_def.bg {
                let color = self.resolve_color(bg.clone())?;
                style = style.bg(color);
            }

            // Apply modifiers
            if style_def.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if style_def.italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if style_def.underlined {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            styles.insert(*component, style);
        }

        Ok(Theme {
            name: self.name,
            styles,
            background_color,
        })
    }

    /// Resolve a color value to a ratatui Color
    fn resolve_color(&self, color_value: ColorValue) -> Result<Color, ThemeError> {
        match color_value {
            ColorValue::Palette(name) => {
                // Look up in palette
                self.palette
                    .get(&name)
                    .map(|&rgb| rgb.into())
                    .ok_or(ThemeError::ColorNotFound(name))
            }
            ColorValue::Direct(color_str) => {
                // Parse as direct color
                parse_direct_color(&color_str)
            }
        }
    }
}

/// Parse a direct color string (hex or named)
fn parse_direct_color(color_str: &str) -> Result<Color, ThemeError> {
    // Try hex color first
    if let Some(hex) = color_str.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16)
                .map_err(|_| ThemeError::InvalidColor(color_str.to_string()))?;
            let g = u8::from_str_radix(&hex[2..4], 16)
                .map_err(|_| ThemeError::InvalidColor(color_str.to_string()))?;
            let b = u8::from_str_radix(&hex[4..6], 16)
                .map_err(|_| ThemeError::InvalidColor(color_str.to_string()))?;
            return Ok(Color::Rgb(r, g, b));
        }
    }

    // Try named colors
    match color_str.to_lowercase().as_str() {
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "white" => Ok(Color::White),
        "gray" | "grey" => Ok(Color::Gray),
        "darkgray" | "darkgrey" | "dark_gray" | "dark_grey" => Ok(Color::DarkGray),
        "lightred" | "light_red" => Ok(Color::LightRed),
        "lightgreen" | "light_green" => Ok(Color::LightGreen),
        "lightyellow" | "light_yellow" => Ok(Color::LightYellow),
        "lightblue" | "light_blue" => Ok(Color::LightBlue),
        "lightmagenta" | "light_magenta" => Ok(Color::LightMagenta),
        "lightcyan" | "light_cyan" => Ok(Color::LightCyan),
        "reset" => Ok(Color::Reset),
        _ => Err(ThemeError::InvalidColor(color_str.to_string())),
    }
}

impl CompiledTheme {
    /// Get a style for a component, falling back to default if not found
    pub fn style(&self, component: Component) -> Style {
        self.styles.get(&component).copied().unwrap_or_default()
    }

    /// Get the background color from the theme, if any
    pub fn get_background_color(&self) -> Option<Color> {
        self.background_color
    }

    // Convenience methods for common styles
    pub fn error_text(&self) -> Style {
        self.style(Component::ErrorText)
    }

    pub fn dim_text(&self) -> Style {
        self.style(Component::DimText)
    }

    pub fn subtle_text(&self) -> Style {
        self.style(Component::DimText)
    }

    pub fn text(&self) -> Style {
        Style::default()
    }
}

impl Default for CompiledTheme {
    fn default() -> Self {
        create_default_theme()
    }
}

/// Create the default theme based on current hardcoded colors
fn create_default_theme() -> CompiledTheme {
    let mut styles = HashMap::new();

    // Define default component styles based on current styles.rs
    styles.insert(Component::StatusBar, Style::default().fg(Color::LightCyan));

    // Input panel styles
    styles.insert(
        Component::InputPanelBorder,
        Style::default().fg(Color::DarkGray),
    );
    styles.insert(
        Component::InputPanelBorderActive,
        Style::default().fg(Color::Yellow),
    );
    styles.insert(
        Component::InputPanelBorderCommand,
        Style::default().fg(Color::Cyan),
    );
    styles.insert(
        Component::InputPanelBorderApproval,
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    );
    styles.insert(
        Component::InputPanelBorderError,
        Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
    );

    // Chat list styles
    styles.insert(
        Component::ChatListBorder,
        Style::default().fg(Color::DarkGray),
    );
    styles.insert(Component::UserMessage, Style::default());
    styles.insert(
        Component::UserMessageRole,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    styles.insert(Component::AssistantMessage, Style::default());
    styles.insert(
        Component::AssistantMessageRole,
        Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD),
    );
    styles.insert(Component::SystemMessage, Style::default());
    styles.insert(
        Component::SystemMessageRole,
        Style::default().fg(Color::Yellow),
    );

    // Tool styles
    styles.insert(Component::ToolCall, Style::default().fg(Color::Cyan));
    styles.insert(Component::ToolCallBorder, Style::default().fg(Color::Cyan));
    styles.insert(Component::ToolCallHeader, Style::default().fg(Color::Cyan));
    styles.insert(Component::ToolCallId, Style::default().fg(Color::DarkGray));
    styles.insert(Component::ToolOutput, Style::default());
    styles.insert(Component::ToolSuccess, Style::default().fg(Color::Green));
    styles.insert(Component::ToolError, Style::default().fg(Color::Red));

    // Thought styles
    styles.insert(Component::ThoughtBox, Style::default().fg(Color::DarkGray));
    styles.insert(Component::ThoughtHeader, Style::default().fg(Color::Gray));
    styles.insert(
        Component::ThoughtBorder,
        Style::default().fg(Color::DarkGray),
    );
    styles.insert(
        Component::ThoughtText,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    );

    // Command styles
    styles.insert(
        Component::CommandPrompt,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    styles.insert(Component::CommandText, Style::default().fg(Color::Cyan));
    styles.insert(Component::CommandSuccess, Style::default().fg(Color::Green));
    styles.insert(Component::CommandError, Style::default().fg(Color::Red));

    // General styles
    styles.insert(Component::ErrorText, Style::default().fg(Color::Red));
    styles.insert(
        Component::ErrorBold,
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    );
    styles.insert(Component::DimText, Style::default().fg(Color::DarkGray));
    styles.insert(
        Component::SelectionHighlight,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );
    styles.insert(
        Component::PlaceholderText,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    );

    // Model info
    styles.insert(
        Component::ModelInfo,
        Style::default().fg(Color::LightMagenta),
    );

    // Notices
    styles.insert(Component::NoticeInfo, Style::default().fg(Color::Blue));
    styles.insert(Component::NoticeWarn, Style::default().fg(Color::Yellow));
    styles.insert(Component::NoticeError, Style::default().fg(Color::Red));

    // Todo priorities
    styles.insert(Component::TodoHigh, Style::default().fg(Color::Red));
    styles.insert(Component::TodoMedium, Style::default().fg(Color::Yellow));
    styles.insert(Component::TodoLow, Style::default().fg(Color::Green));
    styles.insert(Component::TodoPending, Style::default().fg(Color::Blue));
    styles.insert(
        Component::TodoInProgress,
        Style::default().fg(Color::Yellow),
    );
    styles.insert(Component::TodoCompleted, Style::default().fg(Color::Green));

    // Code editing
    styles.insert(Component::CodeAddition, Style::default().fg(Color::Green));
    styles.insert(Component::CodeDeletion, Style::default().fg(Color::Red));
    styles.insert(Component::CodeFilePath, Style::default().fg(Color::Yellow));

    // Popup
    styles.insert(Component::PopupBorder, Style::default().fg(Color::White));
    styles.insert(
        Component::PopupSelection,
        Style::default().fg(Color::Yellow).bg(Color::DarkGray),
    );

    // Markdown styles
    styles.insert(Component::MarkdownH1, Style::default().fg(Color::Cyan));
    styles.insert(Component::MarkdownH2, Style::default().fg(Color::Cyan));
    styles.insert(Component::MarkdownH3, Style::default().fg(Color::Cyan));
    styles.insert(Component::MarkdownH4, Style::default().fg(Color::LightCyan));
    styles.insert(Component::MarkdownH5, Style::default().fg(Color::LightCyan));
    styles.insert(Component::MarkdownH6, Style::default().fg(Color::Gray));
    styles.insert(Component::MarkdownParagraph, Style::default());
    styles.insert(Component::MarkdownBold, Style::default());
    styles.insert(Component::MarkdownItalic, Style::default());
    styles.insert(Component::MarkdownStrikethrough, Style::default());
    styles.insert(
        Component::MarkdownCode,
        Style::default().fg(Color::White).bg(Color::Black),
    );
    styles.insert(
        Component::MarkdownCodeBlock,
        Style::default().bg(Color::Black),
    );
    styles.insert(Component::MarkdownLink, Style::default().fg(Color::Blue));
    styles.insert(
        Component::MarkdownBlockquote,
        Style::default().fg(Color::Green),
    );
    styles.insert(
        Component::MarkdownListBullet,
        Style::default().fg(Color::Gray),
    );
    styles.insert(
        Component::MarkdownListNumber,
        Style::default().fg(Color::LightBlue),
    );

    // Table styles
    styles.insert(
        Component::MarkdownTableBorder,
        Style::default().fg(Color::DarkGray),
    );
    styles.insert(
        Component::MarkdownTableHeader,
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );
    styles.insert(Component::MarkdownTableCell, Style::default());

    CompiledTheme {
        name: "Default".to_string(),
        styles,
        background_color: None, // Default theme has no background color
    }
}
