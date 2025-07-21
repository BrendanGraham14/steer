use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatRenderable, HeightCache};
use ratatui::text::{Line, Span};

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
}

impl ChatRenderable for CommandResponseWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_response_widget_inline() {
        let theme = Theme::default();
        let mut widget = CommandResponseWidget::new("/help".to_string(), "Shows help".to_string());

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Short response should be inline
    }

    #[test]
    fn test_command_response_widget_multiline() {
        let theme = Theme::default();
        let mut widget =
            CommandResponseWidget::new("/help".to_string(), "Line 1\nLine 2\nLine 3".to_string());

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 4); // Command line + 3 response lines
    }
}
