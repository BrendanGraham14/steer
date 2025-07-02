//! Status bar widget for displaying current model and other info

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Style},
    widgets::{Paragraph, Widget},
};

use conductor_core::api::Model;

/// A status bar widget that displays the current model and other status information
pub struct StatusBar<'a> {
    current_model: &'a Model,
    style: Style,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with the given model
    pub fn new(current_model: &'a Model) -> Self {
        Self {
            current_model,
            style: Style::default().fg(Color::LightCyan),
        }
    }

    /// Set the style for the status bar
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let status_text = format!(" {} ", self.current_model);
        let paragraph = Paragraph::new(status_text)
            .style(self.style)
            .alignment(Alignment::Right);
        paragraph.render(area, buf);
    }
}
