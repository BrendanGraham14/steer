use super::{ToolFormatter, helpers::*};
use crate::tui::theme::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use steer_core::app::conversation::ToolResult;
use steer_core::tools::fetch::FetchParams;

pub struct FetchFormatter;

impl ToolFormatter for FetchFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<FetchParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid fetch params",
                theme.error_text(),
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
            Span::styled(format!("({info})"), theme.subtle_text()),
        ]));

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

        let Ok(params) = serde_json::from_value::<FetchParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid fetch params",
                theme.error_text(),
            ))];
        };

        lines.push(Line::from(Span::styled("Fetch Parameters:", theme.text())));
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
                    theme.dim_text(),
                )));
            }
        }

        // Show result if available
        if let Some(result) = result {
            match result {
                ToolResult::Fetch(fetch_result) => {
                    if !fetch_result.content.trim().is_empty() {
                        lines.push(separator_line(wrap_width, theme.dim_text()));

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
                                theme.subtle_text(),
                            )));
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, theme.dim_text()));
                    lines.push(Line::from(Span::styled(
                        error.to_string(),
                        theme.error_text(),
                    )));
                }
                _ => {
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        theme.error_text(),
                    )));
                }
            }
        }

        lines
    }
}
