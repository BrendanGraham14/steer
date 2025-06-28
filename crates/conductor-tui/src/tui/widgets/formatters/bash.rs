use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::bash::BashParams;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde_json::Value;

pub struct BashFormatter;

impl ToolFormatter for BashFormatter {
    fn compact(
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

        // Wrap command if it's too long
        let command_lines: Vec<&str> = params.command.lines().collect();
        if command_lines.len() == 1 && params.command.len() <= wrap_width.saturating_sub(2) {
            // Single line that fits - show inline with prompt
            let mut spans = vec![
                Span::styled("$ ", styles::COMMAND_PROMPT),
                Span::styled(params.command.clone(), styles::COMMAND_TEXT),
            ];

            // Add exit code if error
            if let Some(ToolResult::Error(error)) = result {
                if let Some(exit_code) = error
                    .to_string()
                    .strip_prefix("Exit code: ")
                    .and_then(|s| s.parse::<i32>().ok())
                {
                    spans.push(Span::styled(
                        format!(" (exit {})", exit_code),
                        styles::ERROR_TEXT,
                    ));
                }
            }

            lines.push(Line::from(spans));
        } else {
            // Multi-line or long command - wrap it
            for (i, line) in params.command.lines().enumerate() {
                for (j, wrapped_line) in textwrap::wrap(line, wrap_width.saturating_sub(2))
                    .into_iter()
                    .enumerate()
                {
                    if i == 0 && j == 0 {
                        // First line gets the prompt
                        lines.push(Line::from(vec![
                            Span::styled("$ ", styles::COMMAND_PROMPT),
                            Span::styled(wrapped_line.to_string(), styles::COMMAND_TEXT),
                        ]));
                    } else {
                        // Subsequent lines are indented
                        lines.push(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(wrapped_line.to_string(), styles::COMMAND_TEXT),
                        ]));
                    }
                }
            }

            // Add exit code on a new line if error
            if let Some(ToolResult::Error(error)) = result {
                if let Some(exit_code) = error
                    .to_string()
                    .strip_prefix("Exit code: ")
                    .and_then(|s| s.parse::<i32>().ok())
                {
                    lines.push(Line::from(Span::styled(
                        format!("(exit {})", exit_code),
                        styles::ERROR_TEXT,
                    )));
                }
            }
        }

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
                ToolResult::Bash(bash_result) => {
                    // Show stdout if present
                    if !bash_result.stdout.trim().is_empty() {
                        const MAX_OUTPUT_LINES: usize = 20;
                        let (output_lines, truncated) =
                            truncate_lines(&bash_result.stdout, MAX_OUTPUT_LINES);

                        for line in output_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    bash_result.stdout.lines().count() - MAX_OUTPUT_LINES
                                ),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }

                    // Show stderr if present
                    if !bash_result.stderr.trim().is_empty() {
                        if !bash_result.stdout.trim().is_empty() {
                            lines.push(separator_line(wrap_width, styles::DIM_TEXT));
                        }
                        lines.push(Line::from(Span::styled("[stderr]", styles::ERROR_TEXT)));

                        const MAX_ERROR_LINES: usize = 10;
                        let (error_lines, truncated) =
                            truncate_lines(&bash_result.stderr, MAX_ERROR_LINES);

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
                                    bash_result.stderr.lines().count() - MAX_ERROR_LINES
                                ),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }

                    // Show exit code if non-zero
                    if bash_result.exit_code != 0 {
                        lines.push(Line::from(Span::styled(
                            format!("Exit code: {}", bash_result.exit_code),
                            styles::ERROR_TEXT,
                        )));
                    } else if bash_result.stdout.trim().is_empty()
                        && bash_result.stderr.trim().is_empty()
                    {
                        lines.push(Line::from(Span::styled(
                            "(Command completed successfully with no output)",
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
                ToolResult::Error(error) => {
                    // Show error message
                    const MAX_ERROR_LINES: usize = 10;
                    let error_message = error.to_string();
                    let (error_lines, truncated) = truncate_lines(&error_message, MAX_ERROR_LINES);

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
                                error_message.lines().count() - MAX_ERROR_LINES
                            ),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
                _ => {
                    // Other result types shouldn't appear for bash tool
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        styles::ERROR_TEXT,
                    )));
                }
            }
        }

        lines
    }
}
