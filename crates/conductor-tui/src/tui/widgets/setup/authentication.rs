use crate::tui::state::SetupState;
use crate::tui::theme::{Component, Theme};
use conductor_core::api::ProviderKind;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

pub struct AuthenticationWidget;

impl AuthenticationWidget {
    pub fn render(
        area: Rect,
        buf: &mut Buffer,
        state: &SetupState,
        provider: ProviderKind,
        theme: &Theme,
    ) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Header
                Constraint::Min(10),   // Main content
                Constraint::Length(3), // Error message
                Constraint::Length(3), // Instructions
            ])
            .split(area);

        // Header
        let provider_name = provider.display_name();

        let header = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("Authenticate with {provider_name}"),
                theme.style(Component::SetupHeader),
            )),
        ];

        Paragraph::new(header)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .border_style(theme.style(Component::SetupBorder)),
            )
            .alignment(Alignment::Center)
            .render(chunks[0], buf);

        // Main content
        let mut content = vec![];

        if let Some(oauth_state) = &state.oauth_state {
            // OAuth flow in progress
            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                "OAuth Authentication",
                theme.style(Component::SetupHeader),
            )));
            content.push(Line::from(""));

            if state.oauth_callback_input.is_empty() {
                content.push(Line::from("Please visit this URL in your browser:"));
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    &oauth_state.auth_url,
                    theme.style(Component::SetupUrl),
                )));
                content.push(Line::from(""));
                content.push(Line::from(
                    "After authorizing, you'll be redirected to a page showing a code.",
                ));
                content.push(Line::from(
                    "Copy the ENTIRE code (including the part after the #)",
                ));
                content.push(Line::from("and paste it below:"));
                content.push(Line::from(""));
                content.push(Line::from(vec![
                    Span::styled("Code: ", theme.style(Component::SetupInputLabel)),
                    Span::styled(
                        if state.oauth_callback_input.is_empty() {
                            "_"
                        } else {
                            &state.oauth_callback_input
                        },
                        theme.style(Component::SetupInputValue),
                    ),
                ]));
            } else {
                content.push(Line::from("Processing authorization code..."));
                content.push(Line::from(""));
                content.push(Line::from(vec![
                    Span::styled("Code: ", theme.style(Component::SetupInputLabel)),
                    Span::styled(
                        &state.oauth_callback_input,
                        theme.style(Component::SetupInputValue),
                    ),
                ]));
            }
        } else if provider == ProviderKind::Anthropic
            && state.api_key_input.is_empty()
            && state.oauth_state.is_none()
            && state.auth_providers.get(&provider)
                != Some(&crate::tui::state::AuthStatus::InProgress)
        {
            // Anthropic - show auth options
            content.push(Line::from(""));
            content.push(Line::from("Choose authentication method:"));
            content.push(Line::from(""));
            content.push(Line::from(vec![
                Span::styled("1. ", theme.style(Component::SetupKeyBinding)),
                Span::raw("OAuth Login "),
                Span::styled(
                    "(Recommended for Claude Pro users)",
                    theme.style(Component::SetupHint),
                ),
            ]));
            content.push(Line::from(vec![
                Span::styled("2. ", theme.style(Component::SetupKeyBinding)),
                Span::raw("API Key"),
            ]));
        } else {
            // API key input
            content.push(Line::from(""));
            content.push(Line::from(format!("Enter your {provider_name} API key:")));
            content.push(Line::from(""));

            let masked_key = if state.api_key_input.is_empty() {
                String::from("_")
            } else {
                "*".repeat(state.api_key_input.len())
            };

            content.push(Line::from(vec![
                Span::styled("API Key: ", theme.style(Component::SetupInputLabel)),
                Span::styled(masked_key, theme.style(Component::SetupInputValue)),
            ]));

            if provider == ProviderKind::Anthropic {
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    "Tip: Get your API key from console.anthropic.com",
                    theme.style(Component::SetupHint),
                )));
            }
        }

        Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT)
                    .border_style(theme.style(Component::SetupBorder)),
            )
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true })
            .render(chunks[1], buf);

        // Error message
        if let Some(error) = &state.error_message {
            let error_text = vec![Line::from(Span::styled(
                format!("Error: {error}"),
                theme.style(Component::SetupErrorMessage),
            ))];

            Paragraph::new(error_text)
                .block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .border_style(theme.style(Component::SetupBorder)),
                )
                .alignment(Alignment::Center)
                .render(chunks[2], buf);
        }

        // Instructions
        let instructions = if state.oauth_state.is_some() {
            vec![Line::from(vec![
                Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to cancel"),
            ])]
        } else if provider == ProviderKind::Anthropic && state.api_key_input.is_empty() {
            vec![Line::from(vec![
                Span::raw("Press "),
                Span::styled("1", theme.style(Component::SetupKeyBinding)),
                Span::raw(" or "),
                Span::styled("2", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to select, "),
                Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to go back"),
            ])]
        } else {
            vec![Line::from(vec![
                Span::raw("Type your API key, "),
                Span::styled("Enter", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to submit, "),
                Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to go back"),
            ])]
        };

        Paragraph::new(instructions)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                    .border_style(theme.style(Component::SetupBorder)),
            )
            .alignment(Alignment::Center)
            .render(chunks[3], buf);
    }
}
