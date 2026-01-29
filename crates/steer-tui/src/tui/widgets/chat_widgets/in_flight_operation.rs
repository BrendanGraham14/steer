use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatRenderable, HeightCache};
use ratatui::text::{Line, Span};

/// Widget for in-flight operations with spinner
pub struct InFlightOperationWidget {
    label: String,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl InFlightOperationWidget {
    pub fn new(label: String) -> Self {
        Self {
            label,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }
}

impl ChatRenderable for InFlightOperationWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        if self.rendered_lines.is_some() && self.cache.last_width == width {
            return self.rendered_lines.as_deref().unwrap_or(&[]);
        }

        // Note: The spinner character will be handled by the GutterWidget
        let line = Line::from(Span::styled(
            self.label.clone(),
            theme.style(Component::TodoInProgress),
        ));
        self.rendered_lines = Some(vec![line]);
        self.rendered_lines.as_deref().unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_flight_operation_widget() {
        let theme = Theme::default();
        let mut widget = InFlightOperationWidget::new("Processing...".to_string());

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1); // Always single line
    }
}
