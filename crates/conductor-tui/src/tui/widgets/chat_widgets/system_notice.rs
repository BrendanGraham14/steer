use crate::tui::model::NoticeLevel;
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatWidget, HeightCache};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Widget for system notices (info, warn, error)
pub struct SystemNoticeWidget {
    level: NoticeLevel,
    text: String,
    timestamp: OffsetDateTime,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl SystemNoticeWidget {
    pub fn new(level: NoticeLevel, text: String, timestamp: OffsetDateTime) -> Self {
        Self {
            level,
            text,
            timestamp,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }

    fn render_lines(&mut self, width: u16, theme: &Theme) -> &Vec<Line<'static>> {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let (prefix, component) = match self.level {
                NoticeLevel::Info => ("info: ", Component::NoticeInfo),
                NoticeLevel::Warn => ("warn: ", Component::NoticeWarn),
                NoticeLevel::Error => ("error: ", Component::NoticeError),
            };

            // Format timestamp
            let time_str = self
                .timestamp
                .format(&Rfc3339)
                .unwrap_or_else(|_| "unknown".to_string());

            // Build the full notice text
            let full_text = format!("{}{} ({})", prefix, self.text, time_str);

            // Calculate wrap width
            let wrap_width = width.saturating_sub(2) as usize;

            // Wrap text and build lines
            let mut lines = vec![];
            let wrapped = textwrap::wrap(&full_text, wrap_width);

            for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                let mut line_spans = vec![];

                if i == 0 {
                    // First line - add colored prefix if present
                    if let Some(stripped) = wrapped_line.strip_prefix(prefix) {
                        line_spans.push(Span::styled(prefix, theme.style(component)));
                        line_spans.push(Span::raw(stripped.to_string()));
                    } else {
                        line_spans.push(Span::raw(wrapped_line.to_string()));
                    }
                } else {
                    // Continuation lines
                    line_spans.push(Span::raw(wrapped_line.to_string()));
                }

                lines.push(Line::from(line_spans));
            }

            self.rendered_lines = Some(lines);
        }

        self.rendered_lines.as_ref().unwrap()
    }
}

impl ChatWidget for SystemNoticeWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }

        let lines = self.render_lines(width, theme);
        let height = lines.len();

        self.cache.set(mode, width, height);
        height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let lines = self.render_lines(area.width, theme).clone();
        let bg_style = theme.style(Component::ChatListBackground);
        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(bg_style);

        paragraph.render(area, buf);
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Ensure lines are rendered
        let lines = self.render_lines(area.width, theme);
        if first_line >= lines.len() {
            return;
        }

        // Calculate the slice of lines to render
        let end_line = (first_line + area.height as usize).min(lines.len());
        let visible_lines = &lines[first_line..end_line];

        // Create a paragraph with only the visible lines
        let bg_style = theme.style(Component::ChatListBackground);
        let paragraph = Paragraph::new(visible_lines.to_vec())
            .wrap(Wrap { trim: false })
            .style(bg_style);

        paragraph.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn test_system_notice_widget() {
        let theme = Theme::default();
        let mut widget = SystemNoticeWidget::new(
            NoticeLevel::Info,
            "Test notice".to_string(),
            datetime!(2023-01-01 00:00:00 UTC),
        );

        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, 1); // Should fit on one line with wide width
    }
}
