use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::view::ViewParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use tracing::debug;

pub struct ViewFormatter;

impl ToolFormatter for ViewFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let params = match serde_json::from_value::<ViewParams>(params.clone()) {
            Ok(params) => params,
            Err(e) => {
                debug!("Error parsing view params: {:?}", e);
                return vec![Line::from(Span::styled(
                    "Invalid view params",
                    styles::ERROR_TEXT,
                ))];
            }
        };

        let file_name = std::path::Path::new(&params.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&params.file_path);

        let info = if let Some(result) = result {
            match result {
                ToolResult::FileContent(file_result) => {
                    let line_count = file_result.line_count;
                    let size = format_size(file_result.content.len());
                    if file_result.truncated {
                        format!("{} lines (truncated), {}", line_count, size)
                    } else {
                        format!("{} lines, {}", line_count, size)
                    }
                }
                ToolResult::Error(_) => "failed".to_string(),
                _ => "unexpected result type".to_string(),
            }
        } else {
            "reading...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", file_name), Style::default()),
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

        let Ok(params) = serde_json::from_value::<ViewParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid view params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("File: {}", params.file_path),
            Style::default(),
        )));

        // Add file info if available
        if let Some(offset) = params.offset {
            lines.push(Line::from(Span::styled(
                format!("Starting from line: {}", offset),
                styles::DIM_TEXT,
            )));
        }
        if let Some(limit) = params.limit {
            lines.push(Line::from(Span::styled(
                format!("Max lines: {}", limit),
                styles::DIM_TEXT,
            )));
        }

        // Show file content if we have a result
        if let Some(result) = result {
            match result {
                ToolResult::FileContent(file_result) => {
                    if !file_result.content.trim().is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_PREVIEW_LINES: usize = 20;
                        let content_lines: Vec<&str> = file_result.content.lines().collect();

                        for line in content_lines.iter().take(MAX_PREVIEW_LINES) {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if content_lines.len() > MAX_PREVIEW_LINES {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    content_lines.len() - MAX_PREVIEW_LINES
                                ),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, styles::DIM_TEXT));
                    lines.push(Line::from(Span::styled(
                        error.to_string(),
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
