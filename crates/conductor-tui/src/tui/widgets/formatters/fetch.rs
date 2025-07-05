use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_core::tools::fetch::FetchParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct FetchFormatter;

impl ToolFormatter for FetchFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<FetchParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid fetch params",
                styles::ERROR_TEXT,
            ))];
        };

        let info = match result {
            Some(ToolResult::Fetch(fetch_result)) => format_size(fetch_result.content.len()),
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "fetching...".to_string(),
        };

        let url_display = if params.url.len() > 50 {
            format!("{}...", &params.url[..47])
        } else {
            params.url.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("url={url_display} "), Style::default()),
            Span::styled(format!("({info})"), styles::ITALIC_GRAY),
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

        let Ok(params) = serde_json::from_value::<FetchParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid fetch params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            "Fetch Parameters:",
            styles::TOOL_HEADER,
        )));
        lines.push(Line::from(Span::styled(
            format!("  URL: {}", params.url),
            Style::default(),
        )));

        // Show prompt in wrapped lines
        lines.push(Line::from(Span::styled("  Prompt:", Style::default())));
        for line in params.prompt.lines() {
            for wrapped in textwrap::wrap(line, wrap_width.saturating_sub(4)) {
                lines.push(Line::from(Span::styled(
                    format!("    {wrapped}"),
                    styles::DIM_TEXT,
                )));
            }
        }

        // Show result if available
        if let Some(result) = result {
            match result {
                ToolResult::Fetch(fetch_result) => {
                    if !fetch_result.content.trim().is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_OUTPUT_LINES: usize = 25;
                        let (output_lines, truncated) =
                            truncate_lines(&fetch_result.content, MAX_OUTPUT_LINES);

                        for line in output_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    fetch_result.content.lines().count() - MAX_OUTPUT_LINES
                                ),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, styles::DIM_TEXT));
                    lines.push(Line::from(Span::styled(
                        format!("Error: {error}"),
                        styles::ERROR_TEXT,
                    )));
                }
                _ => {
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
