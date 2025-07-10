use crate::tui::state::{AuthStatus, SetupState};
use crate::tui::theme::{Component, Theme};
use conductor_core::api::ProviderKind;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Widget},
};

pub struct ProviderSelectionWidget;

impl ProviderSelectionWidget {
    pub fn render(area: Rect, buf: &mut Buffer, state: &SetupState, theme: &Theme) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5), // Header
                Constraint::Min(10),   // Provider list
                Constraint::Length(4), // Instructions
            ])
            .split(area);

        // Header
        let header = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Configure authentication",
                theme.style(Component::SetupHeader),
            )),
            Line::from(""),
        ];

        Paragraph::new(header)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .border_style(theme.style(Component::SetupBorder)),
            )
            .alignment(Alignment::Center)
            .render(chunks[0], buf);

        // Provider list
        let providers = state.available_providers();
        let items: Vec<ListItem> = providers
            .iter()
            .enumerate()
            .map(|(i, provider)| {
                let (name, status_icon, status_style) = match provider {
                    ProviderKind::Anthropic => {
                        let status = state.auth_providers.get(provider);
                        let (icon, style) = match status {
                            Some(AuthStatus::OAuthConfigured) => {
                                ("✓", theme.style(Component::SetupStatusActive))
                            }
                            Some(AuthStatus::ApiKeySet) => {
                                ("✓", theme.style(Component::SetupStatusActive))
                            }
                            Some(AuthStatus::InProgress) => {
                                ("⟳", theme.style(Component::SetupStatusInProgress))
                            }
                            _ => ("✗", theme.style(Component::SetupStatusInactive)),
                        };
                        (provider.display_name(), icon, style)
                    }
                    ProviderKind::OpenAI => {
                        let (icon, style) = if matches!(
                            state.auth_providers.get(provider),
                            Some(AuthStatus::ApiKeySet)
                        ) {
                            ("✓", theme.style(Component::SetupStatusActive))
                        } else {
                            ("✗", theme.style(Component::SetupStatusInactive))
                        };
                        (provider.display_name(), icon, style)
                    }
                    ProviderKind::Google => {
                        let (icon, style) = if matches!(
                            state.auth_providers.get(provider),
                            Some(AuthStatus::ApiKeySet)
                        ) {
                            ("✓", theme.style(Component::SetupStatusActive))
                        } else {
                            ("✗", theme.style(Component::SetupStatusInactive))
                        };
                        (provider.display_name(), icon, style)
                    }
                    ProviderKind::XAI => {
                        let (icon, style) = if matches!(
                            state.auth_providers.get(provider),
                            Some(AuthStatus::ApiKeySet)
                        ) {
                            ("✓", theme.style(Component::SetupStatusActive))
                        } else {
                            ("✗", theme.style(Component::SetupStatusInactive))
                        };
                        (provider.display_name(), icon, style)
                    }
                };

                let style = if i == state.provider_cursor {
                    theme.style(Component::SetupProviderSelected)
                } else {
                    theme.style(Component::SetupProviderName)
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {status_icon} "), status_style),
                    Span::styled(name, style),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::LEFT | Borders::RIGHT)
                .border_style(theme.style(Component::SetupBorder)),
        );

        Widget::render(list, chunks[1], buf);

        // Instructions
        let instructions = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("Use "),
                Span::styled("↑/↓", theme.style(Component::SetupKeyBinding)),
                Span::raw(" or "),
                Span::styled("j/k", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to navigate, "),
                Span::styled("Enter", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to select"),
            ]),
            Line::from(vec![
                Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to go back, "),
                Span::styled("S", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to skip setup"),
            ]),
        ];

        Paragraph::new(instructions)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                    .border_style(theme.style(Component::SetupBorder)),
            )
            .alignment(Alignment::Center)
            .render(chunks[2], buf);
    }
}
