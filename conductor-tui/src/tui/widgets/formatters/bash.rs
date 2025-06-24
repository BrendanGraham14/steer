use super::{ToolFormatter, helpers::*};
use conductor_core::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde_json::Value;
use tools::tools::bash::BashParams;

pub struct BashFormatter;

impl ToolFormatter for BashFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<BashParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid bash params",
                styles::ERROR_TEXT,
            ))];
        };

        let cmd_preview = if params.command.len() > 50 {
            format!("{}...", &params.command[..47])
        } else {
            params.command.clone()
        };

        let mut spans = vec![
            Span::styled("$ ", styles::COMMAND_PROMPT),
            Span::styled(cmd_preview, styles::COMMAND_TEXT),
        ];

        // Add exit code if error
        if let Some(ToolResult::Error { error }) = result {
            if let Some(exit_code) = extract_exit_code(error) {
                spans.push(Span::styled(
                    format!(" (exit {})", exit_code),
                    styles::ERROR_TEXT,
                ));
            }
        }

        lines.push(Line::from(spans));
        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<BashParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid bash params",
                styles::ERROR_TEXT,
            ))];
        };

        // Show full command
        for line in params.command.lines() {
            for wrapped_line in textwrap::wrap(line, wrap_width.saturating_sub(2)) {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    Style::default().fg(Color::White),
                )));
            }
        }

        if let Some(timeout) = params.timeout {
            lines.push(Line::from(Span::styled(
                format!("Timeout: {}ms", timeout),
                styles::DIM_TEXT,
            )));
        }

        // Show output if we have results
        if let Some(result) = result {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));

            match result {
                ToolResult::Success { output } => {
                    if output.trim().is_empty() {
                        lines.push(Line::from(Span::styled(
                            "(Command completed successfully with no output)",
                            styles::ITALIC_GRAY,
                        )));
                    } else {
                        const MAX_OUTPUT_LINES: usize = 20;
                        let (output_lines, truncated) = truncate_lines(output, MAX_OUTPUT_LINES);

                        for line in output_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    output.lines().count() - MAX_OUTPUT_LINES
                                ),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }
                }
                ToolResult::Error { error } => {
                    // Show error output
                    const MAX_ERROR_LINES: usize = 10;
                    let (error_lines, truncated) = truncate_lines(error, MAX_ERROR_LINES);

                    for line in error_lines {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                wrapped.to_string(),
                                styles::ERROR_TEXT,
                            )));
                        }
                    }

                    if truncated {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "... ({} more lines)",
                                error.lines().count() - MAX_ERROR_LINES
                            ),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            }
        }

        lines
    }
}
