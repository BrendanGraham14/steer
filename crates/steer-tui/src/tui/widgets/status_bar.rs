//! Status bar widget for displaying current model and other info

use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::tui::theme::{Component, Theme};
use steer_grpc::client_api::ModelId;

/// Optional update badge info
#[derive(Clone, Debug)]
pub enum UpdateBadge<'a> {
    None,
    Available { latest: &'a str },
}

/// A status bar widget that displays the current model and other status information
pub struct StatusBar<'a> {
    current_model: &'a ModelId,
    theme: &'a Theme,
    update: UpdateBadge<'a>,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with the given model and theme
    pub fn new(current_model: &'a ModelId, theme: &'a Theme) -> Self {
        Self {
            current_model,
            theme,
            update: UpdateBadge::None,
        }
    }

    pub fn with_update_badge(mut self, update: UpdateBadge<'a>) -> Self {
        self.update = update;
        self
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = self.theme.style(Component::StatusBar);

        // Left: update badge (if any)
        let left_line = match self.update {
            UpdateBadge::Available { latest } => Line::from(vec![Span::styled(
                format!(" v{latest} available "),
                self.theme.style(Component::NoticeWarn),
            )]),
            UpdateBadge::None => Line::from(" "),
        };
        let left_para = Paragraph::new(left_line)
            .style(style)
            .alignment(Alignment::Left);
        left_para.render(area, buf);

        // Right: current model
        let model_string = format!(
            " {}/{} ",
            self.current_model.provider.storage_key(),
            self.current_model.id
        );
        let right_line = Line::from(vec![Span::raw(model_string)]);
        let right_para = Paragraph::new(right_line)
            .style(style)
            .alignment(Alignment::Right);
        right_para.render(area, buf);
    }
}
