use crate::tui::model::{CommandResponse, TuiCommandResponse};
use crate::tui::theme::{Component, Theme};

use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::chat_widget::{ChatRenderable, HeightCache};
use ratatui::text::{Line, Span};
use steer_core::app::conversation::{CommandResponse as CoreCommandResponse, CompactResult};

/// Widget for command responses (both app commands and tui commands)
pub struct CommandResponseWidget {
    command: String,
    response: CommandResponse,
    cache: HeightCache,
    rendered_lines: Option<Vec<Line<'static>>>,
}

impl CommandResponseWidget {
    pub fn new(command: String, response: CommandResponse) -> Self {
        Self {
            command,
            response,
            cache: HeightCache::new(),
            rendered_lines: None,
        }
    }
}

impl ChatRenderable for CommandResponseWidget {
    fn lines(&mut self, width: u16, _mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        if self.rendered_lines.is_none() || self.cache.last_width != width {
            let mut lines = vec![];
            let wrap_width = width.saturating_sub(2) as usize;

            // Command prompt on its own line
            lines.push(Line::from(vec![
                Span::styled(self.command.clone(), theme.style(Component::CommandPrompt)),
                Span::raw(":"),
            ]));

            // Format response based on type
            match &self.response {
                CommandResponse::Core(core_response) => {
                    match core_response {
                        CoreCommandResponse::Text(text) => {
                            // Simple text wrapping
                            for line in text.lines() {
                                let wrapped = textwrap::wrap(line, wrap_width);
                                if wrapped.is_empty() {
                                    lines.push(Line::from(""));
                                } else {
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            theme.style(Component::CommandText),
                                        )));
                                    }
                                }
                            }
                        }
                        CoreCommandResponse::Compact(result) => match result {
                            CompactResult::Success(summary) => {
                                lines.push(Line::from(vec![
                                    Span::styled("✓ ", theme.style(Component::CommandSuccess)),
                                    Span::styled(
                                        summary.clone(),
                                        theme.style(Component::CommandText),
                                    ),
                                ]));
                            }
                            CompactResult::Cancelled => {
                                lines.push(Line::from(Span::styled(
                                    "Compact cancelled.",
                                    theme.style(Component::CommandError),
                                )));
                            }
                            CompactResult::InsufficientMessages => {
                                lines.push(Line::from(Span::styled(
                                    "Not enough messages to compact.",
                                    theme.style(Component::CommandError),
                                )));
                            }
                        },
                    }
                }

                CommandResponse::Tui(tui_response) => {
                    match tui_response {
                        TuiCommandResponse::Text(text) => {
                            // Simple text wrapping
                            for line in text.lines() {
                                let wrapped = textwrap::wrap(line, wrap_width);
                                if wrapped.is_empty() {
                                    lines.push(Line::from(""));
                                } else {
                                    for wrapped_line in wrapped {
                                        lines.push(Line::from(Span::styled(
                                            wrapped_line.to_string(),
                                            theme.style(Component::CommandText),
                                        )));
                                    }
                                }
                            }
                        }

                        TuiCommandResponse::Theme { name } => {
                            lines.push(Line::from(vec![
                                Span::styled(
                                    "✓ Theme changed to ",
                                    theme.style(Component::CommandText),
                                ),
                                Span::styled(
                                    format!("'{name}'"),
                                    theme.style(Component::CommandSuccess),
                                ),
                            ]));
                        }

                        TuiCommandResponse::ListThemes(themes) => {
                            if themes.is_empty() {
                                lines.push(Line::from(Span::styled(
                                    "No themes found.",
                                    theme.style(Component::CommandText),
                                )));
                            } else {
                                lines.push(Line::from(Span::styled(
                                    "Available themes:",
                                    theme.style(Component::CommandText),
                                )));
                                for theme_name in themes {
                                    lines.push(Line::from(vec![
                                        Span::raw("  • "),
                                        Span::styled(
                                            theme_name.clone(),
                                            theme.style(Component::CommandSuccess),
                                        ),
                                    ]));
                                }
                            }
                        }

                        TuiCommandResponse::ListMcpServers(servers) => {
                            use steer_core::session::state::McpConnectionState;

                            if servers.is_empty() {
                                lines.push(Line::from(Span::styled(
                                    "No MCP servers configured.",
                                    theme.style(Component::CommandText),
                                )));
                            } else {
                                lines.push(Line::from(Span::styled(
                                    "MCP Server Status:",
                                    theme.style(Component::CommandText),
                                )));
                                lines.push(Line::from("")); // Empty line for spacing

                                for server in servers {
                                    // Server name and status
                                    // let status_icon = match &server.state {
                                    //     McpConnectionState::Connecting => "⏳",
                                    //     McpConnectionState::Connected { .. } => "✅",
                                    //     McpConnectionState::Failed { .. } => "❌",
                                    // };

                                    lines.push(Line::from(vec![
                                        // Span::raw(format!("{} ", status_icon)),
                                        Span::styled(
                                            server.server_name.clone(),
                                            theme.style(Component::CommandPrompt),
                                        ),
                                    ]));

                                    // Connection state details
                                    match &server.state {
                                        McpConnectionState::Connecting => {
                                            lines.push(Line::from(vec![
                                                Span::raw("   Status: "),
                                                Span::styled(
                                                    "Connecting...",
                                                    theme.style(Component::DimText),
                                                ),
                                            ]));
                                        }
                                        McpConnectionState::Connected { tool_names } => {
                                            lines.push(Line::from(vec![
                                                Span::raw("   Status: "),
                                                Span::styled(
                                                    "Connected",
                                                    theme.style(Component::CommandSuccess),
                                                ),
                                            ]));

                                            if !tool_names.is_empty() {
                                                lines.push(Line::from(vec![
                                                    Span::raw("   Tools: "),
                                                    Span::styled(
                                                        format!("{} available", tool_names.len()),
                                                        theme.style(Component::CommandText),
                                                    ),
                                                ]));

                                                // Show first few tool names
                                                let display_count = tool_names.len().min(5);
                                                for tool in &tool_names[..display_count] {
                                                    lines.push(Line::from(vec![
                                                        Span::raw("     • "),
                                                        Span::styled(
                                                            tool.clone(),
                                                            theme.style(Component::ToolCall),
                                                        ),
                                                    ]));
                                                }
                                                if tool_names.len() > 5 {
                                                    lines.push(Line::from(vec![
                                                        Span::raw("     "),
                                                        Span::styled(
                                                            format!(
                                                                "... and {} more",
                                                                tool_names.len() - 5
                                                            ),
                                                            theme.style(Component::CommandText),
                                                        ),
                                                    ]));
                                                }
                                            }
                                        }
                                        McpConnectionState::Failed { error } => {
                                            lines.push(Line::from(vec![
                                                Span::raw("   Status: "),
                                                Span::styled(
                                                    "Failed",
                                                    theme.style(Component::CommandError),
                                                ),
                                            ]));

                                            // Wrap error message
                                            let error_prefix = "   Error: ";
                                            let error_wrap_width =
                                                wrap_width.saturating_sub(error_prefix.len());
                                            let wrapped_error =
                                                textwrap::wrap(error, error_wrap_width);

                                            for (i, wrapped_line) in
                                                wrapped_error.iter().enumerate()
                                            {
                                                if i == 0 {
                                                    lines.push(Line::from(vec![
                                                        Span::raw(error_prefix),
                                                        Span::styled(
                                                            wrapped_line.to_string(),
                                                            theme.style(Component::ErrorText),
                                                        ),
                                                    ]));
                                                } else {
                                                    lines.push(Line::from(Span::styled(
                                                        format!("          {wrapped_line}"),
                                                        theme.style(Component::ErrorText),
                                                    )));
                                                }
                                            }
                                        }
                                    }

                                    // Transport info
                                    use steer_core::tools::McpTransport;
                                    let transport_desc = match &server.transport {
                                        McpTransport::Stdio { command, args } => {
                                            format!("stdio: {} {}", command, args.join(" "))
                                        }
                                        McpTransport::Tcp { host, port } => {
                                            format!("tcp: {host}:{port}")
                                        }
                                        McpTransport::Unix { path } => {
                                            format!("unix: {path}")
                                        }
                                        McpTransport::Sse { url, .. } => {
                                            format!("sse: {url}")
                                        }
                                        McpTransport::Http { url, .. } => {
                                            format!("http: {url}")
                                        }
                                    };

                                    lines.push(Line::from(vec![
                                        Span::raw("   Transport: "),
                                        Span::styled(
                                            transport_desc,
                                            theme.style(Component::CommandText),
                                        ),
                                    ]));

                                    lines.push(Line::from("")); // Empty line between servers
                                }
                            }
                        }
                    }
                }
            }

            self.rendered_lines = Some(lines);
        }

        self.rendered_lines.as_ref().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::tui::model::TuiCommandResponse;

    use super::*;

    #[test]
    fn test_command_response_widget_inline() {
        let theme = Theme::default();
        let mut widget = CommandResponseWidget::new(
            "/help".to_string(),
            TuiCommandResponse::Text("Shows help".to_string()).into(),
        );

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 2); // Command line + response line (always multi-line now)
    }

    #[test]
    fn test_command_response_widget_multiline() {
        let theme = Theme::default();
        let mut widget = CommandResponseWidget::new(
            "/help".to_string(),
            TuiCommandResponse::Text("Line 1\nLine 2\nLine 3".to_string()).into(),
        );

        let height = widget.lines(80, ViewMode::Compact, &theme).len();
        assert_eq!(height, 4); // Command line + 3 response lines
    }
}
