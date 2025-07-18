use ratatui::{buffer::Buffer, layout::Rect};

use crate::tui::{
    theme::{Component, Theme},
    widgets::{ChatWidget, GutterWidget, ViewMode},
};

/// Composite wrapper for one chat list row
pub struct RowWidget {
    gutter: GutterWidget,
    body: Box<dyn ChatWidget + Send + Sync>,
    /// Cached total height for the body
    body_height: Option<usize>,
}

impl RowWidget {
    pub fn new(gutter: GutterWidget, body: Box<dyn ChatWidget + Send + Sync>) -> Self {
        Self {
            gutter,
            body,
            body_height: None,
        }
    }
}

impl ChatWidget for RowWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        // Height is determined by the body widget
        // Account for the 2-column gutter when calculating body width
        let body_width = width.saturating_sub(2);
        let height = self.body.height(mode, body_width, theme);
        self.body_height = Some(height);
        height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, mode: ViewMode, theme: &Theme) {
        if area.width < 2 || area.height == 0 {
            return; // Not enough space for gutter + body
        }

        // Split area into gutter and body
        let gutter_area = Rect {
            x: area.x,
            y: area.y,
            width: 2,
            height: area.height.min(1), // Gutter only renders on first line
        };

        let body_area = Rect {
            x: area.x + 2,
            y: area.y,
            width: area.width - 2,
            height: area.height,
        };

        // Render gutter on the first line
        self.gutter.render(gutter_area, buf, mode, theme);

        // For multi-line content, fill remaining gutter lines with themed spaces
        if area.height > 1 {
            let continuation_style = theme.style(Component::ChatListBackground);
            for dy in 1..area.height {
                let y = area.y + dy;
                for dx in 0..2 {
                    buf[(area.x + dx, y)]
                        .set_style(continuation_style)
                        .set_symbol(" ");
                }
            }
        }

        // Render body in the remaining space
        self.body.render(body_area, buf, mode, theme);
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        if area.width < 2 || area.height == 0 {
            return;
        }

        // Render gutter glyph only if we're showing the first line
        if first_line == 0 {
            let gutter_area = Rect {
                x: area.x,
                y: area.y,
                width: 2,
                height: 1,
            };
            self.gutter.render(gutter_area, buf, mode, theme);
        }

        // Render the body with appropriate offset
        let body_area = Rect {
            x: area.x + 2,
            y: area.y,
            width: area.width - 2,
            height: area.height,
        };
        self.body
            .render_partial(body_area, buf, mode, theme, first_line);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::RowWidget;
    use crate::tui::{
        theme::Theme,
        widgets::{ChatWidget, GutterWidget, ParagraphWidget, RoleGlyph, ViewMode},
    };

    #[test]
    fn test_row_widget_height_delegation() {
        let theme = Theme::default();
        let gutter = GutterWidget::new(RoleGlyph::User);
        let body = Box::new(ParagraphWidget::from_text(
            "Test content".to_string(),
            &theme,
        ));
        let mut row = RowWidget::new(gutter, body);

        // Row height should match body height
        let height = row.height(ViewMode::Compact, 20, &theme);
        assert_eq!(height, 1); // Single line of text

        // With narrow width (accounting for 2-char gutter)
        let narrow_height = row.height(ViewMode::Compact, 8, &theme);
        assert!(narrow_height > 1); // Should wrap
    }

    #[test]
    fn test_row_widget_render_layout() {
        let theme = Theme::default();
        let gutter = GutterWidget::new(RoleGlyph::Assistant);
        let body = Box::new(ParagraphWidget::from_text(
            "Hello world".to_string(),
            &theme,
        ));
        let mut row = RowWidget::new(gutter, body);

        // Create a test terminal
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 20, 2);
                row.render(area, f.buffer_mut(), ViewMode::Compact, &theme);
            })
            .unwrap();

        // Check layout
        let buffer = terminal.backend().buffer();

        // Gutter area (first 2 columns)
        assert_eq!(buffer[(0, 0)].symbol(), "◀");
        assert_eq!(buffer[(1, 0)].symbol(), " ");

        // Body area starts at column 2
        assert_eq!(buffer[(2, 0)].symbol(), "H");
        assert_eq!(buffer[(3, 0)].symbol(), "e");

        // Second line should have empty gutter
        assert_eq!(buffer[(0, 1)].symbol(), " ");
        assert_eq!(buffer[(1, 1)].symbol(), " ");
    }
}
