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
const OUTER_MARGIN: u16 = PADDING;

pub struct RowWidget {
    body: Box<dyn ChatRenderable + Send + Sync>,
    row_background: Option<Style>,
    accent_style: Option<Style>,
    has_padding_lines: bool,
    separator_above: bool,
    separator_style: Option<Style>,
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
            separator_above: false,
            separator_style: None,
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

    pub fn with_separator_above(mut self, style: Style) -> Self {
        self.separator_above = true;
        self.separator_style = Some(style);
        self
    }
}
impl ChatRenderable for RowWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let theme_key = theme.name.clone();
        let has_accent = self.accent_style.is_some();
        let accent_width = if has_accent { ACCENT_WIDTH } else { 0 };
        let bg_color = self.row_background.and_then(|s| s.bg);
        let has_row_bg = bg_color.is_some();
        let outer_margin = if has_row_bg { OUTER_MARGIN } else { 0 };
        let bubble_width = width.saturating_sub(outer_margin * 2);
        let left_inset = if has_row_bg { 0 } else { PADDING };
        let gap_after_accent = if has_accent { PADDING } else { 0 };
        let right_padding_width = PADDING;

        let body_width = bubble_width
            .saturating_sub(left_inset)
            .saturating_sub(accent_width)
            .saturating_sub(gap_after_accent)
            .saturating_sub(right_padding_width);
        let body_lines = self.body.lines(body_width, mode, theme);

        let mut hasher = DefaultHasher::new();
        body_lines.len().hash(&mut hasher);
        if let Some(first) = body_lines.first() {
            for s in &first.spans {
                s.content.hash(&mut hasher);
            }
        }
        if body_lines.len() > 1
            && let Some(last) = body_lines.last()
        {
            for s in &last.spans {
                s.content.hash(&mut hasher);
            }
        }
        let body_fp = hasher.finish();

        if self.last_width == width
            && self.last_mode == mode
            && self.last_theme_name == theme_key
            && self.last_body_fingerprint == body_fp
            && let Some(ref lines) = self.cached_lines
        {
            return lines;
        }

        let accent_width = if has_accent { ACCENT_WIDTH as usize } else { 0 };
        let padding_style = if let Some(bg) = bg_color {
            Style::default().bg(bg)
        } else {
            Style::default()
        };

        let make_padding_line = |w: u16| -> Line<'static> {
            let mut acc = Vec::with_capacity(4);
            if has_row_bg && outer_margin > 0 {
                acc.push(Span::styled(
                    " ".repeat(outer_margin as usize),
                    Style::default(),
                ));
            } else if left_inset > 0 {
                acc.push(Span::styled(" ".repeat(left_inset as usize), padding_style));
            }
            if let Some(accent) = self.accent_style {
                acc.push(Span::styled("▌", accent));
            }
            if gap_after_accent > 0 {
                acc.push(Span::styled(
                    " ".repeat(gap_after_accent as usize),
                    padding_style,
                ));
            }
            let fill_width = if has_row_bg {
                bubble_width
                    .saturating_sub(accent_width as u16)
                    .saturating_sub(gap_after_accent) as usize
            } else {
                w.saturating_sub(left_inset)
                    .saturating_sub(accent_width as u16)
                    .saturating_sub(gap_after_accent) as usize
            };
            acc.push(Span::styled(" ".repeat(fill_width), padding_style));
            if has_row_bg && outer_margin > 0 {
                acc.push(Span::styled(
                    " ".repeat(outer_margin as usize),
                    Style::default(),
                ));
            }
            Line::from(acc)
        };

        let extra_lines = if self.has_padding_lines { 2 } else { 0 }
            + if self.separator_above { 1 } else { 0 };
        let mut lines = Vec::with_capacity(body_lines.len() + extra_lines);

        if self.separator_above {
            let sep_style = self.separator_style.unwrap_or_default();
            let rule_width = width.saturating_sub(PADDING * 2) as usize;
            let mut sep_spans = Vec::with_capacity(3);
            sep_spans.push(Span::raw(" ".repeat(PADDING as usize)));
            sep_spans.push(Span::styled("─".repeat(rule_width), sep_style));
            sep_spans.push(Span::raw(" ".repeat(PADDING as usize)));
            lines.push(Line::from(sep_spans));
        }

        if self.has_padding_lines {
            lines.push(make_padding_line(width));
        }

        for line in body_lines {
            let spans = line.spans.clone();
            let mut acc = Vec::with_capacity(6 + spans.len());

            if has_row_bg && outer_margin > 0 {
                acc.push(Span::styled(
                    " ".repeat(outer_margin as usize),
                    Style::default(),
                ));
            } else if left_inset > 0 {
                acc.push(Span::styled(" ".repeat(left_inset as usize), padding_style));
            }
            if let Some(accent) = self.accent_style {
                acc.push(Span::styled("▌", accent));
            }
            if gap_after_accent > 0 {
                acc.push(Span::styled(
                    " ".repeat(gap_after_accent as usize),
                    padding_style,
                ));
            }

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

            let used_width = if has_row_bg {
                accent_width + (gap_after_accent as usize) + content_width
            } else {
                (left_inset as usize) + accent_width + (gap_after_accent as usize) + content_width
            };
            let remaining = if has_row_bg {
                bubble_width as usize
            } else {
                width as usize
            }
            .saturating_sub(used_width);
            let fill = remaining.saturating_sub(right_padding_width as usize);
            if fill > 0 {
                acc.push(Span::styled(" ".repeat(fill), padding_style));
            }
            acc.push(Span::styled(
                " ".repeat(right_padding_width as usize),
                padding_style,
            ));

            if has_row_bg && outer_margin > 0 {
                acc.push(Span::styled(
                    " ".repeat(outer_margin as usize),
                    Style::default(),
                ));
            }

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
        self.cached_lines.as_deref().unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use steer_grpc::client_api::{AssistantContent, Message, MessageData};

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
        let mut terminal = Terminal::new(backend).expect("create test terminal");

        let area = Rect::new(0, 0, 30, 2);
        terminal
            .draw(|f| {
                let lines = row.lines(area.width, ViewMode::Compact, &theme);
                let para = ratatui::widgets::Paragraph::new(lines.to_vec());
                f.render_widget(para, area);
            })
            .expect("draw row widget");

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
