use super::{ToolFormatter, helpers::*};
use crate::tui::theme::{Component, Theme};
use ratatui::text::{Line, Span};
use serde_json::Value;
use steer_core::app::conversation::ToolResult;

pub struct DefaultFormatter;

impl ToolFormatter for DefaultFormatter {
    fn compact(
        &self,
        _params: &Value,
        _result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            "Unknown tool",
            theme.style(Component::ErrorText),
        ))]
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::from(Span::styled(
            "Tool Parameters:",
            theme.style(Component::ToolCallHeader),
        )));

        // Show parameters as pretty-printed JSON
        let pretty_params = serde_json::to_string_pretty(params).unwrap_or_default();
        for line in pretty_params.lines() {
            let wrapped_lines = textwrap::wrap(line, wrap_width);
            for wrapped_line in wrapped_lines {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    theme.style(Component::DimText),
                )));
            }
        }

        // Show result if available
        if let Some(result) = result {
            lines.push(separator_line(wrap_width, theme.style(Component::DimText)));

            // Show the result as pretty-printed JSON
            match serde_json::to_string_pretty(result) {
                Ok(pretty_result) => {
                    const MAX_LINES: usize = 10;
                    let (output_lines, truncated) = truncate_lines(&pretty_result, MAX_LINES);

                    for line in output_lines {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::raw(wrapped.to_string())));
                        }
                    }

                    if truncated {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "... ({} more lines)",
                                pretty_result.lines().count() - MAX_LINES
                            ),
                            theme
                                .style(Component::DimText)
                                .add_modifier(ratatui::style::Modifier::ITALIC),
                        )));
                    }
                }
                Err(_) => {
                    lines.push(Line::from(Span::styled(
                        "(Unable to display result)",
                        theme
                            .style(Component::DimText)
                            .add_modifier(ratatui::style::Modifier::ITALIC),
                    )));
                }
            }
        }

        lines
    }
}
