use super::{
    ToolFormatter,
    helpers::{json_preview, separator_line, tool_error_user_message, truncate_middle, wrap_lines},
};
use crate::tui::theme::Theme;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use steer_grpc::client_api::ToolResult;
use textwrap;

/// Fallback formatter for MCP/external tools (names starting with "mcp__")
/// Displays parameters and payload/error in a generic way.
pub struct ExternalFormatter;

impl ToolFormatter for ExternalFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        // Show a one-liner with param preview and short payload/error summary.
        let mut spans = Vec::new();

        // Param preview
        let preview = json_preview(params, 30);
        spans.push(Span::styled(preview.clone(), theme.dim_text()));

        if let Some(result) = result {
            match result {
                ToolResult::External(ext) => {
                    let payload_preview = truncate_middle(&ext.payload, 40);
                    spans.push(Span::raw(" → "));
                    spans.push(Span::styled(payload_preview, Style::default()));
                }
                ToolResult::Error(err) => {
                    spans.push(Span::raw(" ✗ "));
                    spans.push(Span::styled(
                        tool_error_user_message(err).into_owned(),
                        theme.error_text(),
                    ));
                }
                _ => {}
            }
        }

        vec![Line::from(spans)]
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Parameters block
        lines.push(Line::from(Span::styled("Parameters:", theme.text())));

        let pretty_params = serde_json::to_string_pretty(params).unwrap_or_default();
        for line in wrap_lines(pretty_params.lines(), wrap_width) {
            lines.push(Line::from(Span::styled(line, theme.dim_text())));
        }

        // Result block
        if let Some(result) = result {
            lines.push(separator_line(wrap_width, theme.dim_text()));
            match result {
                ToolResult::External(ext) => {
                    // Attempt to pretty-print JSON payload with 2-space indent
                    if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&ext.payload) {
                        if let Ok(pretty) = serde_json::to_string_pretty(&json_val) {
                            for ln in wrap_lines(pretty.lines(), wrap_width) {
                                lines.push(Line::from(Span::raw(ln)));
                            }
                        } else {
                            // Fallback to raw text if serialization fails
                            for wrapped in textwrap::wrap(&ext.payload, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }
                    } else {
                        // Non-JSON payload – render raw text
                        for wrapped in textwrap::wrap(&ext.payload, wrap_width) {
                            lines.push(Line::from(Span::raw(wrapped.to_string())));
                        }
                    }
                }
                ToolResult::Error(err) => {
                    for wrapped in textwrap::wrap(&tool_error_user_message(err), wrap_width) {
                        lines.push(Line::from(Span::styled(
                            wrapped.to_string(),
                            theme.error_text(),
                        )));
                    }
                }
                _ => {}
            }
        }

        lines
    }
}
