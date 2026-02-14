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
    current_agent: Option<&'a str>,
    theme: &'a Theme,
    update: UpdateBadge<'a>,
    context_remaining_percent: Option<f64>,
}

impl<'a> StatusBar<'a> {
    /// Create a new status bar with the given model and theme
    pub fn new(
        current_model: &'a ModelId,
        current_agent: Option<&'a str>,
        theme: &'a Theme,
    ) -> Self {
        Self {
            current_model,
            current_agent,
            theme,
            update: UpdateBadge::None,
            context_remaining_percent: None,
        }
    }

    pub fn with_update_badge(mut self, update: UpdateBadge<'a>) -> Self {
        self.update = update;
        self
    }

    pub fn with_context_remaining_percent(
        mut self,
        context_remaining_percent: Option<f64>,
    ) -> Self {
        self.context_remaining_percent = context_remaining_percent;
        self
    }
}

fn format_context_remaining_percent(context_remaining_percent: f64) -> String {
    let clamped = context_remaining_percent.clamp(0.0, 100.0);
    format!("ctx {clamped:.1}%")
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let style = self.theme.style(Component::StatusBar);

        // Left: update badge (if any) + current agent
        let mut left_spans = Vec::new();
        if let UpdateBadge::Available { latest } = self.update {
            left_spans.push(Span::styled(
                format!(" v{latest} available "),
                self.theme.style(Component::NoticeWarn),
            ));
        }
        let agent_text = match self.current_agent {
            Some(agent) => format!(" {agent} "),
            None => " -- ".to_string(),
        };
        left_spans.push(Span::raw(agent_text));
        let left_line = Line::from(left_spans);
        let left_para = Paragraph::new(left_line)
            .style(style)
            .alignment(Alignment::Left);
        left_para.render(area, buf);

        // Right: context remaining (if any) + current model
        let mut right_spans = Vec::new();
        right_spans.push(Span::raw(" "));
        if let Some(context_remaining_percent) = self.context_remaining_percent {
            right_spans.push(Span::raw(format_context_remaining_percent(
                context_remaining_percent,
            )));
            right_spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
        }
        right_spans.push(Span::raw(format!(
            "{}/{} ",
            self.current_model.provider.storage_key(),
            self.current_model.id
        )));
        let right_line = Line::from(right_spans);
        let right_para = Paragraph::new(right_line)
            .style(style)
            .alignment(Alignment::Right);
        right_para.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::format_context_remaining_percent;

    #[test]
    fn format_context_remaining_percent_clamps_and_formats() {
        assert_eq!(format_context_remaining_percent(88.126), "ctx 88.1%");
        assert_eq!(format_context_remaining_percent(101.0), "ctx 100.0%");
        assert_eq!(format_context_remaining_percent(-5.0), "ctx 0.0%");
    }
}
