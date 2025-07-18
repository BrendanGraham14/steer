//! Gutter widget for rendering role glyphs, spinners, and hover states
//!
//! This module provides a widget for the left-hand gutter area that displays
//! role indicators, animated spinners, and hover states.

use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::ChatWidget;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// Role glyph types for different message types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleGlyph {
    User,      // ▶
    Assistant, // ◀
    Tool,      // ⚙
    Meta,      // •
}

impl RoleGlyph {
    /// Get the display character for this role
    pub fn as_char(&self) -> &'static str {
        match self {
            RoleGlyph::User => "▶",
            RoleGlyph::Assistant => "◀",
            RoleGlyph::Tool => "⚙",
            RoleGlyph::Meta => "•",
        }
    }

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

/// Left-hand glyph area widget (2 cells wide)
pub struct GutterWidget {
    role: RoleGlyph,
    spinner: Option<char>,
    hovered: bool,
}

impl GutterWidget {
    pub fn new(role: RoleGlyph) -> Self {
        Self {
            role,
            spinner: None,
            hovered: false,
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
}

impl ChatWidget for GutterWidget {
    fn height(&mut self, _mode: ViewMode, _width: u16, _theme: &Theme) -> usize {
        // Gutter is always 1 line tall
        1usize
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Get base style for the role
        let mut style = theme.style(self.role.theme_component());

        // For spinner, apply special styling based on role
        if self.spinner.is_some() {
            style = match self.role {
                RoleGlyph::Tool => theme
                    .style(Component::ToolCall)
                    .add_modifier(Modifier::BOLD),
                RoleGlyph::Meta => theme
                    .style(Component::TodoInProgress)
                    .add_modifier(Modifier::BOLD),
                _ => style.add_modifier(Modifier::BOLD),
            };
        }

        // Add bold modifier if hovered
        if self.hovered {
            style = style.add_modifier(Modifier::BOLD);
        }

        // Build the gutter content
        let content = if let Some(spinner_char) = self.spinner {
            // Show spinner instead of role glyph
            format!("{spinner_char} ")
        } else {
            // Show role glyph with space padding
            format!("{} ", self.role.as_char())
        };

        // Create a line with the styled content
        let line = Line::from(Span::styled(content, style));
        let paragraph = Paragraph::new(vec![line]);

        // Render into the area
        paragraph.render(area, buf);
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _mode: ViewMode,
        _theme: &Theme,
        _first_line: usize,
    ) {
        self.render(area, buf, _mode, _theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn test_role_glyph_display() {
        assert_eq!(RoleGlyph::User.as_char(), "▶");
        assert_eq!(RoleGlyph::Assistant.as_char(), "◀");
        assert_eq!(RoleGlyph::Tool.as_char(), "⚙");
        assert_eq!(RoleGlyph::Meta.as_char(), "•");
    }

    #[test]
    fn test_gutter_widget_height() {
        let theme = Theme::default();
        let mut gutter = GutterWidget::new(RoleGlyph::User);

        // Gutter height is always 1
        assert_eq!(gutter.height(ViewMode::Compact, 10, &theme), 1);
        assert_eq!(gutter.height(ViewMode::Detailed, 100, &theme), 1);
    }

    #[test]
    fn test_gutter_widget_render_bounds() {
        let theme = Theme::default();
        let mut gutter = GutterWidget::new(RoleGlyph::User).with_hover(true);

        // Create a test terminal
        let backend = TestBackend::new(10, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 2, 1);
                gutter.render(area, f.buffer_mut(), ViewMode::Compact, &theme);
            })
            .unwrap();

        // Check that content is only in the 2x1 area
        let buffer = terminal.backend().buffer();

        // First two cells should have the gutter content
        let cell0 = &buffer[(0, 0)];
        let cell1 = &buffer[(1, 0)];
        assert_eq!(cell0.symbol(), "▶");
        assert_eq!(cell1.symbol(), " ");

        // Rest should be empty
        for x in 2..10 {
            let cell = &buffer[(x, 0)];
            assert_eq!(cell.symbol(), " ");
        }
    }

    #[test]
    fn test_spinner_overlay() {
        let theme = Theme::default();
        let mut gutter = GutterWidget::new(RoleGlyph::Tool).with_spinner('⠋');

        // Create a test terminal
        let backend = TestBackend::new(2, 1);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 2, 1);
                gutter.render(area, f.buffer_mut(), ViewMode::Compact, &theme);
            })
            .unwrap();

        // Should show spinner instead of role glyph
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "⠋");
        assert_eq!(buffer[(1, 0)].symbol(), " ");
    }
}
