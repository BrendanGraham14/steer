use ratatui::text::{Line, Span};

use crate::tui::{
    theme::Theme,
    widgets::{ChatRenderable, Gutter, ViewMode},
};

/// Composite wrapper for one chat list row
pub struct RowWidget {
    gutter: Gutter,
    body: Box<dyn ChatRenderable + Send + Sync>,
    /// Cached total height for the body
    cached_lines: Option<Vec<Line<'static>>>,
}

impl RowWidget {
    pub fn new(gutter: Gutter, body: Box<dyn ChatRenderable + Send + Sync>) -> Self {
        Self {
            gutter,
            body,
            cached_lines: None,
        }
    }
}
impl ChatRenderable for RowWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        if self.cached_lines.is_some() && self.cached_lines.as_ref().unwrap().is_empty() {
            return self.cached_lines.as_ref().unwrap();
        }

        let gutter_span = self.gutter.span(theme);
        let body_width = width.saturating_sub(self.gutter.width);
        let body_lines = self.body.lines(body_width, mode, theme);

        // Prefix first line with gutter, rest with spaces
        self.cached_lines = Some(
            body_lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let spans = line.spans.clone();
                    if i == 0 {
                        let mut first_line_spans = vec![gutter_span.clone()];
                        first_line_spans.extend(spans);
                        Line::from(first_line_spans)
                    } else {
                        let mut other_line_spans = vec![Span::raw(format!(
                            "{:width$}",
                            "",
                            width = self.gutter.width as usize
                        ))];
                        other_line_spans.extend(spans);
                        Line::from(other_line_spans)
                    }
                })
                .collect(),
        );

        self.cached_lines.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use conductor_core::app::{Message, conversation::AssistantContent};
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::RowWidget;
    use crate::tui::{
        theme::Theme,
        widgets::{ChatRenderable, Gutter, RoleGlyph, ViewMode, chat_widgets::MessageWidget},
    };

    #[test]
    fn test_row_widget_render_layout() {
        let theme = Theme::default();
        let gutter = Gutter::new(RoleGlyph::Assistant);
        let message = Message::Assistant {
            content: vec![AssistantContent::Text {
                text: "Hello world\nHow are you?".to_string(),
            }],
            id: "1".to_string(),
            parent_message_id: None,
            timestamp: 0,
        };
        let message_widget = MessageWidget::new(message);
        let mut row = RowWidget::new(gutter, Box::new(message_widget));

        // Create a test terminal
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        let area = Rect::new(0, 0, 20, 2);
        terminal
            .draw(|f| {
                let lines = row.lines(area.width, ViewMode::Compact, &theme);
                let para = ratatui::widgets::Paragraph::new(lines.to_vec());
                f.render_widget(para, area);
            })
            .unwrap();

        // Check layout
        let buffer = terminal.backend().buffer();
        let buffer_lines = buffer.content.chunks(buffer.area.width as usize);
        let expected_line_symbols = ["◀ Hello world", "  How are you?"];

        for (i, line) in buffer_lines.enumerate() {
            if i >= expected_line_symbols.len() {
                break;
            }
            let line_content = line.iter().map(|cell| cell.symbol()).collect::<String>();
            let expected = expected_line_symbols[i];
            assert_eq!(&line_content[..expected.len()], expected);
        }
    }
}
