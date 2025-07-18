use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatWidget, HeightCache};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Widget for in-flight operations with spinner
pub struct InFlightOperationWidget {
    label: String,
    cache: HeightCache,
}

impl InFlightOperationWidget {
    pub fn new(label: String) -> Self {
        Self {
            label,
            cache: HeightCache::new(),
        }
    }
}

impl ChatWidget for InFlightOperationWidget {
    fn height(&mut self, mode: ViewMode, width: u16, _theme: &Theme) -> usize {
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }

        // In-flight operations are always single line
        let height = 1usize;
        self.cache.set(mode, width, height);
        height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Note: The spinner character will be handled by the GutterWidget
        let line = Line::from(Span::styled(
            self.label.clone(),
            theme.style(Component::TodoInProgress),
        ));

        let bg_style = theme.style(Component::ChatListBackground);
        let paragraph = Paragraph::new(vec![line])
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
        _first_line: usize,
    ) {
        self.render(area, buf, _mode, theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_in_flight_operation_widget() {
        let theme = Theme::default();
        let mut widget = InFlightOperationWidget::new("Processing...".to_string());

        let height = widget.height(ViewMode::Compact, 80, &theme);
        assert_eq!(height, 1); // Always single line
    }
}
