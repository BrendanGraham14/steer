use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::tui::{
    theme::Theme,
    widgets::{ChatRenderable, ViewMode},
};

const ACCENT_WIDTH: u16 = 1;
const PADDING: u16 = 2;

pub struct RowWidget {
    body: Box<dyn ChatRenderable + Send + Sync>,
    row_background: Option<Style>,
    accent_style: Option<Style>,
    has_padding_lines: bool,
    cached_lines: Option<Vec<Line<'static>>>,
    last_width: u16,
    last_mode: ViewMode,
    last_theme_name: String,
    last_body_fingerprint: u64,
}

impl RowWidget {
    pub fn new(body: Box<dyn ChatRenderable + Send + Sync>) -> Self {
        Self {
            body,
            row_background: None,
            accent_style: None,
            has_padding_lines: false,
            cached_lines: None,
            last_width: 0,
            last_mode: ViewMode::Compact,
            last_theme_name: String::new(),
            last_body_fingerprint: 0,
        }
    }

    pub fn with_accent(mut self, style: Style) -> Self {
        self.accent_style = Some(style);
        self
    }

    pub fn with_row_background(mut self, style: Style) -> Self {
        self.row_background = Some(style);
        self
    }

    pub fn with_padding_lines(mut self) -> Self {
        self.has_padding_lines = true;
        self
    }
}
impl ChatRenderable for RowWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let theme_key = theme.name.clone();
        let accent_width = if self.accent_style.is_some() {
            ACCENT_WIDTH
        } else {
            0
        };

        let body_width = width
            .saturating_sub(accent_width)
            .saturating_sub(PADDING * 2);
        let body_lines = self.body.lines(body_width, mode, theme);

        let mut hasher = DefaultHasher::new();
        body_lines.len().hash(&mut hasher);
        if let Some(first) = body_lines.first() {
            for s in &first.spans {
                s.content.hash(&mut hasher);
            }
        }
        if body_lines.len() > 1 {
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
            && self.last_body_fingerprint == body_fp
        {
            if let Some(ref lines) = self.cached_lines {
                return lines;
            }
        }

        let has_accent = self.accent_style.is_some();
        let accent_width = if has_accent { ACCENT_WIDTH as usize } else { 0 };
        let bg_color = self.row_background.and_then(|s| s.bg);
        let padding_style = if let Some(bg) = bg_color {
            Style::default().bg(bg)
        } else {
            Style::default()
        };

        let make_padding_line = |w: u16| -> Line<'static> {
            let mut acc = Vec::with_capacity(3);
            acc.push(Span::styled(" ".repeat(PADDING as usize), padding_style));
            if let Some(accent) = self.accent_style {
                acc.push(Span::styled("▌", accent));
            }
            let fill_width = w
                .saturating_sub(PADDING)
                .saturating_sub(accent_width as u16) as usize;
            acc.push(Span::styled(" ".repeat(fill_width), padding_style));
            Line::from(acc)
        };

        let extra_lines = if self.has_padding_lines { 2 } else { 0 };
        let mut lines = Vec::with_capacity(body_lines.len() + extra_lines);

        if self.has_padding_lines {
            lines.push(make_padding_line(width));
        }

        for line in body_lines.iter() {
            let spans = line.spans.clone();
            let mut acc = Vec::with_capacity(5 + spans.len());

            acc.push(Span::styled(" ".repeat(PADDING as usize), padding_style));
            if let Some(accent) = self.accent_style {
                acc.push(Span::styled("▌", accent));
            }
            acc.push(Span::styled(" ".repeat(PADDING as usize), padding_style));

            let content_width: usize = spans.iter().map(|s| s.content.width()).sum();

            for span in spans {
                let styled_span = if span.style.bg.is_none() {
                    if let Some(bg) = bg_color {
                        Span::styled(span.content, span.style.bg(bg))
                    } else {
                        span
                    }
                } else {
                    span
                };
                acc.push(styled_span);
            }

            let used_width = (PADDING as usize) + accent_width + (PADDING as usize) + content_width;
            let remaining = (width as usize).saturating_sub(used_width);
            let right_padding = PADDING as usize;
            let fill = remaining.saturating_sub(right_padding);
            if fill > 0 {
                acc.push(Span::styled(" ".repeat(fill), padding_style));
            }
            acc.push(Span::styled(" ".repeat(right_padding), padding_style));

            lines.push(Line::from(acc));
        }

        if self.has_padding_lines {
            lines.push(make_padding_line(width));
        }

        self.last_width = width;
        self.last_mode = mode;
        self.last_theme_name = theme_key;
        self.last_body_fingerprint = body_fp;
        self.cached_lines = Some(lines);
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
        theme::{Component, Theme},
        widgets::{ChatRenderable, ViewMode, chat_widgets::MessageWidget},
    };

    #[test]
    fn test_row_widget_render_layout() {
        let theme = Theme::default();
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
        let accent_style = theme.style(Component::AssistantMessageAccent);
        let mut row = RowWidget::new(Box::new(message_widget)).with_accent(accent_style);

        let backend = TestBackend::new(30, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        let area = Rect::new(0, 0, 30, 2);
        terminal
            .draw(|f| {
                let lines = row.lines(area.width, ViewMode::Compact, &theme);
                let para = ratatui::widgets::Paragraph::new(lines.to_vec());
                f.render_widget(para, area);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let buffer_lines = buffer.content.chunks(buffer.area.width as usize);
        let expected_line_symbols = ["  ▌  Hello world", "  ▌  How are you?"];

        for (i, line) in buffer_lines.enumerate() {
            if i >= expected_line_symbols.len() {
                break;
            }
            let line_content = line.iter().map(|cell| cell.symbol()).collect::<String>();
            let expected = expected_line_symbols[i];
            assert_eq!(
                &line_content[..expected.len()],
                expected,
                "Line {i} mismatch"
            );
        }
    }

    #[test]
    fn test_row_widget_with_padding_lines() {
        let theme = Theme::default();
        let message = Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "Test".to_string(),
                }],
            },
            id: "1".to_string(),
            parent_message_id: None,
            timestamp: 0,
        };
        let message_widget = MessageWidget::new(message);
        let accent_style = theme.style(Component::UserMessageAccent);
        let mut row = RowWidget::new(Box::new(message_widget))
            .with_accent(accent_style)
            .with_padding_lines();

        let lines = row.lines(30, ViewMode::Compact, &theme);
        assert_eq!(
            lines.len(),
            3,
            "Should have top padding + content + bottom padding"
        );
    }
}
