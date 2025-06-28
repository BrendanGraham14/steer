use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use ratatui::text::{Line, Span};
use serde_json::Value;

pub struct DefaultFormatter;

impl ToolFormatter for DefaultFormatter {
    fn compact(
        &self,
        _params: &Value,
        _result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled("Unknown tool", styles::ERROR_TEXT))]
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::from(Span::styled(
            "Tool Parameters:",
            styles::TOOL_HEADER,
        )));

        // Show parameters as pretty-printed JSON
        let pretty_params = serde_json::to_string_pretty(params).unwrap_or_default();
        for line in pretty_params.lines() {
            let wrapped_lines = textwrap::wrap(line, wrap_width);
            for wrapped_line in wrapped_lines {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    styles::DIM_TEXT,
                )));
            }
        }

        // Show result if available
        if let Some(result) = result {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));

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
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
                Err(_) => {
                    lines.push(Line::from(Span::styled(
                        "(Unable to display result)",
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        }

        lines
    }
}
