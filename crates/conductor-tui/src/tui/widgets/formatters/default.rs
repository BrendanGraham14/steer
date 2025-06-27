use super::{ToolFormatter, helpers::*};
use conductor_core::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::text::{Line, Span};
use serde_json::Value;

pub struct DefaultFormatter;

impl ToolFormatter for DefaultFormatter {
    fn compact(
        &self,
        params: &Value,
        _result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let preview = json_preview(params, 60);

        lines.push(Line::from(vec![Span::styled(
            format!("Tool: {}", preview),
            styles::DIM_TEXT,
        )]));

        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Show parameters
        if let Ok(json) = serde_json::to_string_pretty(params) {
            for line in json.lines() {
                let wrapped_lines = textwrap::wrap(line, wrap_width);
                for wrapped_line in wrapped_lines {
                    lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        styles::DIM_TEXT,
                    )));
                }
            }
        }

        // Show result if available
        if let Some(result) = result {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));

            match result {
                ToolResult::Success { output } => {
                    if output.trim().is_empty() {
                        lines.push(Line::from(Span::styled("(No output)", styles::ITALIC_GRAY)));
                    } else {
                        const MAX_LINES: usize = 10;
                        let (output_lines, truncated) = truncate_lines(output, MAX_LINES);

                        for line in output_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more lines)", output.lines().count() - MAX_LINES),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }
                }
                ToolResult::Error { error } => {
                    lines.push(Line::from(Span::styled(
                        format!("Error: {}", error),
                        styles::ERROR_TEXT,
                    )));
                }
            }
        }

        lines
    }
}
