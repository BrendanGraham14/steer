use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatWidget, HeightCache};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Widget for command responses (both app commands and tui commands)
pub struct CommandResponseWidget {
    command: String,
    response: String,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl CommandResponseWidget {
    pub fn new(command: String, response: String) -> Self {
        Self {
            command,
            response,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }

    fn render_lines(&mut self, width: u16, theme: &Theme) -> &Vec<Line<'static>> {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let mut lines = vec![];
            let wrap_width = width.saturating_sub(2) as usize;

            // Split response into lines
            let response_lines: Vec<&str> = self.response.lines().collect();

            if response_lines.is_empty()
                || (response_lines.len() == 1 && response_lines[0].len() <= 50)
            {
                // Single short line - render inline
                let spans = vec![
                    Span::styled(self.command.clone(), theme.style(Component::CommandPrompt)),
                    Span::raw(": "),
                    Span::styled(self.response.clone(), theme.style(Component::CommandText)),
                ];
                lines.push(Line::from(spans));
            } else {
                // Multi-line or long response
                lines.push(Line::from(vec![
                    Span::styled(self.command.clone(), theme.style(Component::CommandPrompt)),
                    Span::raw(":"),
                ]));

                // Add response lines with proper wrapping
                for line in response_lines {
                    let wrapped = textwrap::wrap(line, wrap_width);
                    if wrapped.is_empty() {
                        lines.push(Line::from(""));
                    } else {
                        for wrapped_line in wrapped {
                            lines.push(Line::from(Span::styled(
                                wrapped_line.to_string(),
                                theme.style(Component::CommandText),
                            )));
                        }
                    }
                }
            }

            self.rendered_lines = Some(lines);
        }

        self.rendered_lines.as_ref().unwrap()
    }
}

impl ChatWidget for CommandResponseWidget {
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
    fn test_command_response_widget_inline() {
        let theme = Theme::default();
        let mut widget = CommandResponseWidget::new("/help".to_string(), "Shows help".to_string());

        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, 1); // Short response should be inline
    }

    #[test]
    fn test_command_response_widget_multiline() {
        let theme = Theme::default();
        let mut widget =
            CommandResponseWidget::new("/help".to_string(), "Line 1\nLine 2\nLine 3".to_string());

        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, 4); // Command line + 3 response lines
    }
}
