//! Status bar widget for displaying current model and other info

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    widgets::{Paragraph, Widget},
};

use crate::tui::theme::{Component, Theme};
use steer_core::config::model::ModelId;

/// A status bar widget that displays the current model and other status information
pub struct StatusBar<'a> {
    current_model: &'a ModelId,
    theme: &'a Theme,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with the given model and theme
    pub fn new(current_model: &'a ModelId, theme: &'a Theme) -> Self {
        Self {
            current_model,
            theme,
        }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let status_text = format!(
            " {}/{} ",
            self.current_model.0.storage_key(),
            self.current_model.1
        );
        let style = self.theme.style(Component::StatusBar);
        let paragraph = Paragraph::new(status_text)
            .style(style)
            .alignment(Alignment::Right);
        paragraph.render(area, buf);
    }
}
