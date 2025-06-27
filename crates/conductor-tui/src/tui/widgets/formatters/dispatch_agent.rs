use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_core::tools::dispatch_agent::DispatchAgentParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct DispatchAgentFormatter;

impl ToolFormatter for DispatchAgentFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<DispatchAgentParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid agent params",
                styles::ERROR_TEXT,
            ))];
        };

        let preview = if params.prompt.len() > 60 {
            format!("{}...", &params.prompt[..57])
        } else {
            params.prompt.clone()
        };

        let info = if let Some(ToolResult::Success { output }) = result {
            let line_count = output.lines().count();
            format!("{} lines", line_count)
        } else if let Some(ToolResult::Error { .. }) = result {
            "failed".to_string()
        } else {
            "running...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("task='{}' ", preview), Style::default()),
            Span::styled(format!("({})", info), styles::ITALIC_GRAY),
        ]));

        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<DispatchAgentParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid agent params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled("Agent Task:", styles::TOOL_HEADER)));
        for line in params.prompt.lines() {
            for wrapped_line in textwrap::wrap(line, wrap_width) {
                lines.push(Line::from(Span::styled(
                    wrapped_line.to_string(),
                    Style::default(),
                )));
            }
        }

        // Show output if we have results
        if let Some(ToolResult::Success { output }) = result {
            if !output.trim().is_empty() {
                lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                const MAX_OUTPUT_LINES: usize = 30;
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
        } else if let Some(ToolResult::Error { error }) = result {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));
            lines.push(Line::from(Span::styled(
                format!("Error: {}", error),
                styles::ERROR_TEXT,
            )));
        }

        lines
    }
}
