use crate::tui::state::{AuthStatus, SetupState};
use crate::tui::theme::{Component, Theme};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

pub struct CompletionWidget;

impl CompletionWidget {
    pub fn render(area: Rect, buf: &mut Buffer, state: &SetupState, theme: &Theme) {
        let mut lines = vec![];

        // Add padding
        let vertical_padding = (area.height.saturating_sub(12)) / 2;
        for _ in 0..vertical_padding {
            lines.push(Line::from(""));
        }

        // Success message
        lines.push(Line::from(Span::styled(
            "✓ Setup Complete!",
            theme.style(Component::SetupSuccessIcon),
        )));
        lines.push(Line::from(""));

        // Show authenticated providers
        let authenticated_providers: Vec<_> = state
            .auth_providers
            .iter()
            .filter(|(_, status)| {
                matches!(status, AuthStatus::ApiKeySet | AuthStatus::OAuthConfigured)
            })
            .map(|(provider, _)| provider.display_name())
            .collect();

        if !authenticated_providers.is_empty() {
            lines.push(Line::from(Span::styled(
                "Authenticated Providers:",
                theme
                    .style(Component::SetupHeader)
                    .add_modifier(Modifier::BOLD),
            )));
            for provider in authenticated_providers {
                lines.push(Line::from(format!("  • {provider}")));
            }
        } else {
            lines.push(Line::from("No providers authenticated yet."));
        }

        lines.push(Line::from(""));
        lines.push(Line::from("Your authentication has been configured."));
        lines.push(Line::from(""));

        // Instructions
        lines.push(Line::from(vec![
            Span::raw("Press "),
            Span::styled("Enter", theme.style(Component::SetupKeyBinding)),
            Span::raw(" to start using Conductor"),
        ]));

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme.style(Component::SetupBorder))
                    .title(" Setup Complete "),
            )
            .alignment(Alignment::Center);

        Widget::render(paragraph, area, buf);
    }
}
