use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatWidget, HeightCache};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Widget for slash input display
pub struct SlashInputWidget {
    raw: String,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl SlashInputWidget {
    pub fn new(raw: String) -> Self {
        Self {
            raw,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }

    fn render_lines(&mut self, width: u16, theme: &Theme) -> &Vec<Line<'static>> {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let wrap_width = width.saturating_sub(2) as usize;
            let wrapped = textwrap::wrap(&self.raw, wrap_width);

            let lines: Vec<Line<'static>> = wrapped
                .into_iter()
                .map(|line| {
                    Line::from(Span::styled(
                        line.to_string(),
                        theme.style(Component::CommandPrompt),
                    ))
                })
                .collect();

            self.rendered_lines = Some(lines);
        }

        self.rendered_lines.as_ref().unwrap()
    }
}

impl ChatWidget for SlashInputWidget {
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
    use super::*;

    #[test]
    fn test_slash_input_widget() {
        let theme = Theme::default();
        let mut widget = SlashInputWidget::new("/model gpt-4".to_string());

        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, 1); // Should fit on one line
    }
}
