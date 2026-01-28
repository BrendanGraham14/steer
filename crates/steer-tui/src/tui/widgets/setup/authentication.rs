use crate::tui::state::SetupState;
use crate::tui::theme::{Component, Theme};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use steer_grpc::client_api::{AuthProgress, ProviderId, provider};

pub struct AuthenticationWidget;

impl AuthenticationWidget {
    pub fn render(
        area: Rect,
        buf: &mut Buffer,
        state: &SetupState,
        provider_id: ProviderId,
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
        let provider_config = state.registry.get(&provider_id);
        let provider_name = provider_config
            .map(|c| c.name.as_str())
            .unwrap_or("Unknown Provider");
        let is_openai = provider_id == provider::openai();

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

        match state.auth_progress.as_ref() {
            Some(AuthProgress::OAuthStarted { auth_url }) => {
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    "OAuth Authentication",
                    theme.style(Component::SetupHeader),
                )));
                content.push(Line::from(""));
                content.push(Line::from("Please visit this URL in your browser:"));
                content.push(Line::from(""));
                content.push(Line::from(Span::styled(
                    auth_url,
                    theme.style(Component::SetupUrl),
                )));
                content.push(Line::from(""));
                if is_openai {
                    content.push(Line::from(
                        "After authorizing, you'll be redirected to http://localhost:1455/auth/callback.",
                    ));
                    content.push(Line::from(
                        "If nothing happens, copy the full URL from your browser",
                    ));
                    content.push(Line::from("and paste it below:"));
                } else {
                    content.push(Line::from(
                        "After authorizing, you'll be redirected to a page showing a code.",
                    ));
                    content.push(Line::from(
                        "Copy the full URL or the code (including the part after the #)",
                    ));
                    content.push(Line::from("and paste it below:"));
                }
                content.push(Line::from(""));
                content.push(Line::from(vec![
                    Span::styled("Callback: ", theme.style(Component::SetupInputLabel)),
                    Span::styled(
                        if state.auth_input.is_empty() {
                            "_"
                        } else {
                            &state.auth_input
                        },
                        theme.style(Component::SetupInputValue),
                    ),
                ]));
            }
            Some(AuthProgress::NeedInput { prompt }) => {
                content.push(Line::from(""));
                content.push(Line::from(prompt.clone()));
                content.push(Line::from(""));

                let masked_input = if state.auth_input.is_empty() {
                    String::from("_")
                } else {
                    "*".repeat(state.auth_input.len())
                };

                content.push(Line::from(vec![
                    Span::styled("Input: ", theme.style(Component::SetupInputLabel)),
                    Span::styled(masked_input, theme.style(Component::SetupInputValue)),
                ]));

                if provider_id == provider::anthropic() {
                    content.push(Line::from(""));
                    content.push(Line::from(Span::styled(
                        "Tip: Get your API key from console.anthropic.com",
                        theme.style(Component::SetupHint),
                    )));
                }
            }
            Some(AuthProgress::InProgress { message }) => {
                content.push(Line::from(""));
                content.push(Line::from(message.clone()));
            }
            Some(AuthProgress::Complete) => {
                content.push(Line::from(""));
                content.push(Line::from("Authentication complete."));
            }
            Some(AuthProgress::Error { message }) => {
                content.push(Line::from(""));
                content.push(Line::from(format!("Error: {}", message)));
            }
            None => {
                content.push(Line::from(""));
                content.push(Line::from("Starting authentication..."));
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
        let instructions = match state.auth_progress.as_ref() {
            Some(AuthProgress::OAuthStarted { .. }) | Some(AuthProgress::NeedInput { .. }) => {
                vec![Line::from(vec![
                    Span::raw("Type or paste input, "),
                    Span::styled("Enter", theme.style(Component::SetupKeyBinding)),
                    Span::raw(" to submit, "),
                    Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                    Span::raw(" to cancel"),
                ])]
            }
            _ => vec![Line::from(vec![
                Span::styled("Esc", theme.style(Component::SetupKeyBinding)),
                Span::raw(" to go back"),
            ])],
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
