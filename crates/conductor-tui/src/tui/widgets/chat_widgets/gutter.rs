//! Gutter widget for rendering role glyphs, spinners, and hover states
//!
//! This module provides a widget for the left-hand gutter area that displays
//! role indicators, animated spinners, and hover states.

use crate::tui::theme::{Component, Theme};
use ratatui::{style::Modifier, text::Span};
use std::fmt;

/// Role glyph types for different message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleGlyph {
    User,      // ▶
    Assistant, // ◀
    Tool,      // ⚙
    Meta,      // •
}

impl fmt::Display for RoleGlyph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let glyph = match self {
            RoleGlyph::User => "▶",
            RoleGlyph::Assistant => "◀",
            RoleGlyph::Tool => "⚙",
            RoleGlyph::Meta => "•",
        };
        write!(f, "{glyph}")
    }
}

impl RoleGlyph {
    /// Get the theme component for this role
    pub fn theme_component(&self) -> Component {
        match self {
            RoleGlyph::User => Component::UserMessageRole,
            RoleGlyph::Assistant => Component::AssistantMessageRole,
            RoleGlyph::Tool => Component::ToolCall,
            RoleGlyph::Meta => Component::DimText,
        }
    }
}

pub struct Gutter {
    role: RoleGlyph,
    spinner: Option<char>,
    hovered: bool,
    pub width: u16,
}

impl Gutter {
    pub fn new(role: RoleGlyph) -> Self {
        Self {
            role,
            spinner: None,
            hovered: false,
            width: 2,
        }
    }

    pub fn with_spinner(mut self, spinner: char) -> Self {
        self.spinner = Some(spinner);
        self
    }

    pub fn with_hover(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    pub fn with_role(mut self, role: RoleGlyph) -> Self {
        self.role = role;
        self
    }

    // Just return a styled Span, not Lines
    pub fn span(&self, theme: &Theme) -> Span<'static> {
        let mut style = theme.style(self.role.theme_component());

        if self.spinner.is_some() {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.hovered {
            style = style.add_modifier(Modifier::BOLD);
        }

        let char = if let Some(spinner) = self.spinner {
            spinner.to_string()
        } else {
            self.role.to_string()
        };

        // -1 because char takes up one column
        let content = format!("{char}{:width$}", "", width = (self.width - 1) as usize);
        Span::styled(content, style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_glyph_display() {
        assert_eq!(RoleGlyph::User.to_string(), "▶");
        assert_eq!(RoleGlyph::Assistant.to_string(), "◀");
        assert_eq!(RoleGlyph::Tool.to_string(), "⚙");
        assert_eq!(RoleGlyph::Meta.to_string(), "•");
    }
}
