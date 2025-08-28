use ratatui::text::{Line, Span};

use crate::tui::{
    theme::Theme,
    widgets::{ChatRenderable, Gutter, ViewMode},
};

/// Composite wrapper for one chat list row
pub struct RowWidget {
    gutter: Gutter,
    body: Box<dyn ChatRenderable + Send + Sync>,
    /// Cached decorated lines for current (width, mode, theme, gutter-state, body-state)
    cached_lines: Option<Vec<Line<'static>>>,
    last_width: u16,
    last_mode: ViewMode,
    last_theme_name: String,
    last_gutter_key: String,
    last_body_fingerprint: u64,
}

impl RowWidget {
    pub fn new(gutter: Gutter, body: Box<dyn ChatRenderable + Send + Sync>) -> Self {
        Self {
            gutter,
            body,
            cached_lines: None,
            last_width: 0,
            last_mode: ViewMode::Compact,
            last_theme_name: String::new(),
            last_gutter_key: String::new(),
            last_body_fingerprint: 0,
        }
    }
}
impl ChatRenderable for RowWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Include theme and gutter state in cache key
        let theme_key = theme.name.clone();
        let gutter_key = self.gutter.cache_key();

        let gutter_span = self.gutter.span(theme);
        let body_width = width.saturating_sub(self.gutter.width);
        let body_lines = self.body.lines(body_width, mode, theme);

        // Compute a lightweight fingerprint of the body's rendered lines
        let mut hasher = DefaultHasher::new();
        body_lines.len().hash(&mut hasher);
        if let Some(first) = body_lines.first() {
            for s in &first.spans {
                s.content.hash(&mut hasher);
            }
        }
        if body_lines.len() > 1 {
            // hash last line too for basic change detection
            if let Some(last) = body_lines.last() {
                for s in &last.spans {
                    s.content.hash(&mut hasher);
                }
            }
        }
        let body_fp = hasher.finish();

        if self.last_width == width
            && self.last_mode == mode
            && self.last_theme_name == theme_key
            && self.last_gutter_key == gutter_key
            && self.last_body_fingerprint == body_fp
        {
            if let Some(ref lines) = self.cached_lines {
                return lines;
            }
        }

        // Prefix first line with gutter, rest with spaces
        let decorated: Vec<Line<'static>> = body_lines
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let spans = line.spans.clone();
                if i == 0 {
                    let mut first_line_spans = Vec::with_capacity(1 + spans.len());
                    first_line_spans.push(gutter_span.clone());
                    first_line_spans.extend(spans);
                    Line::from(first_line_spans)
                } else {
                    let mut other_line_spans = Vec::with_capacity(1 + spans.len());
                    other_line_spans.push(Span::raw(" ".repeat(self.gutter.width as usize)));
                    other_line_spans.extend(spans);
                    Line::from(other_line_spans)
                }
            })
            .collect();

        self.last_width = width;
        self.last_mode = mode;
        self.last_theme_name = theme_key;
        self.last_gutter_key = gutter_key;
        self.last_body_fingerprint = body_fp;
        self.cached_lines = Some(decorated);
        self.cached_lines.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use steer_core::app::{
        Message,
        conversation::{AssistantContent, MessageData},
    };

    use super::RowWidget;
    use crate::tui::{
        theme::Theme,
        widgets::{ChatRenderable, Gutter, RoleGlyph, ViewMode, chat_widgets::MessageWidget},
    };

    #[test]
    fn test_row_widget_render_layout() {
        let theme = Theme::default();
        let gutter = Gutter::new(RoleGlyph::Assistant);
        let message = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "Hello world\nHow are you?".to_string(),
                }],
            },
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
        let expected_line_symbols = ["  ◀ Hello world", "    How are you?"];

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
