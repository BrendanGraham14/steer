use super::{
    ToolFormatter,
    helpers::{separator_line, tool_error_user_message, truncate_middle},
};
use crate::tui::theme::{Component, Theme};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use steer_grpc::client_api::ToolResult;
use steer_tools::tools::grep::GrepParams;

pub struct GrepFormatter;

impl ToolFormatter for GrepFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<GrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid grep params",
                theme.style(Component::ErrorText),
            ))];
        };

        let mut spans = vec![Span::styled(params.pattern.clone(), Style::default())];

        // Add include filter if present
        if let Some(include) = &params.include {
            spans.push(Span::raw(" in "));
            spans.push(Span::styled(include.clone(), Style::default()));
        }

        // Add path if not current directory
        if let Some(path) = &params.path {
            if path != "." && !path.is_empty() {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("path={}", truncate_middle(path, 30)),
                    Style::default(),
                ));
            }
        }

        // Add count info from results
        let info = match result {
            Some(ToolResult::Search(search_result)) => {
                let match_count = search_result.matches.len();
                if match_count == 0 {
                    "no matches".to_string()
                } else {
                    format!("{match_count} matches")
                }
            }
            Some(ToolResult::Error(_)) => "error".to_string(),
            _ => "searching...".to_string(),
        };

        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("in {}", params.path.as_deref().unwrap_or(".")),
            theme.style(Component::DimText),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("({info})"),
            theme
                .style(Component::DimText)
                .add_modifier(Modifier::ITALIC),
        ));

        lines.push(Line::from(spans));
        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<GrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid grep params",
                theme.style(Component::ErrorText),
            ))];
        };

        // Show search parameters
        lines.push(Line::from(Span::styled(
            format!("Pattern: {}", params.pattern),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        if let Some(include) = &params.include {
            lines.push(Line::from(Span::styled(
                format!("Include: {include}"),
                Style::default(),
            )));
        }

        if let Some(path) = &params.path {
            if path != "." && !path.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("Path: {path}"),
                    Style::default(),
                )));
            }
        }

        // Show output if we have results
        if let Some(result) = result {
            match result {
                ToolResult::Search(search_result) => {
                    if search_result.matches.is_empty() {
                        lines.push(Line::from(Span::styled(
                            "No matches found",
                            theme
                                .style(Component::DimText)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    } else {
                        lines.push(separator_line(wrap_width, theme.style(Component::DimText)));

                        const MAX_MATCHES: usize = 15;
                        let truncated = search_result.matches.len() > MAX_MATCHES;
                        let display_matches = search_result.matches.iter().take(MAX_MATCHES);

                        for match_item in display_matches {
                            let line = format!(
                                "{}:{}: {}",
                                match_item.file_path,
                                match_item.line_number,
                                match_item.line_content
                            );
                            // Wrap long lines
                            let wrapped_lines = textwrap::wrap(&line, wrap_width);
                            for (i, wrapped_line) in wrapped_lines.iter().enumerate() {
                                if i == 0 {
                                    lines.push(Line::from(Span::raw(wrapped_line.to_string())));
                                } else {
                                    // Indent continuation lines
                                    lines.push(Line::from(vec![
                                        Span::raw("  "),
                                        Span::raw(wrapped_line.to_string()),
                                    ]));
                                }
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more matches)",
                                    search_result.matches.len() - MAX_MATCHES
                                ),
                                theme
                                    .style(Component::DimText)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                    lines.push(Line::from(Span::styled(
                        tool_error_user_message(error).into_owned(),
                        theme.style(Component::ErrorText),
                    )));
                }
                _ => {
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        theme.style(Component::ErrorText),
                    )));
                }
            }
        }

        lines
    }
}
