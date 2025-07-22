use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatRenderable, HeightCache};
use ratatui::text::{Line, Span};

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
}

impl ChatRenderable for SlashInputWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slash_input_widget() {
        let theme = Theme::default();
        let mut widget = SlashInputWidget::new("/model gpt-4".to_string());

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Should fit on one line
    }
}
