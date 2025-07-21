use crate::tui::model::NoticeLevel;
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatRenderable, HeightCache};
use ratatui::text::{Line, Span};
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
}

impl ChatRenderable for SystemNoticeWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
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

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Should fit on one line with wide width
    }
}
