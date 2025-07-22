use super::{ToolFormatter, helpers::*};
use crate::tui::theme::{Component, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use std::path::Path;
use steer_core::app::conversation::ToolResult;
use steer_tools::tools::view::ViewParams;

pub struct ViewFormatter;
const MAX_LINES: usize = 100;

impl ToolFormatter for ViewFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<ViewParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid view params",
                theme.style(Component::ErrorText),
            ))];
        };

        let file_name = Path::new(&params.file_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&params.file_path);

        let mut spans = vec![Span::styled(file_name.to_string(), Style::default())];

        // Add line range info if present
        if params.offset.is_some() || params.limit.is_some() {
            let offset = params.offset.map_or(1, |o| o + 1);
            let limit = params.limit.unwrap_or(0);
            let end_line = if limit > 0 { offset + limit - 1 } else { 0 };

            if limit > 0 {
                spans.push(Span::styled(
                    format!(" [{offset}-{end_line}]"),
                    Style::default(),
                ));
            } else {
                spans.push(Span::styled(format!(" [{offset}+]"), Style::default()));
            }
        }

        // Add count info from results
        let info = extract_view_info(result);
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("({info})"),
            theme
                .style(Component::DimText)
                .add_modifier(Modifier::ITALIC),
        ));

        lines.push(Line::from(spans));
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

        let Ok(params) = serde_json::from_value::<ViewParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid view params",
                theme.style(Component::ErrorText),
            ))];
        };

        // Show file path
        lines.push(Line::from(Span::styled(
            format!("File: {}", params.file_path),
            theme
                .style(Component::CodeFilePath)
                .add_modifier(Modifier::BOLD),
        )));

        // Show line range if specified
        if params.offset.is_some() || params.limit.is_some() {
            lines.push(Line::from(Span::styled(
                format!("Lines: {}", format_line_range(params.offset, params.limit)),
                theme.style(Component::DimText),
            )));
        }

        // Show output if we have results
        if let Some(result) = result {
            match result {
                ToolResult::FileContent(file_content) => {
                    if !file_content.content.is_empty() {
                        lines.push(separator_line(wrap_width, theme.style(Component::DimText)));

                        let (output_lines, truncated) =
                            truncate_lines(&file_content.content, MAX_LINES);

                        // Trim line number & tab
                        let trimmed_lines: Vec<&str> = output_lines
                            .iter()
                            .map(|line| if line.len() > 6 { &line[6..] } else { "" })
                            .collect();

                        for line in trimmed_lines {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if truncated {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "... ({} more lines)",
                                    file_content.content.lines().count() - MAX_LINES
                                ),
                                theme
                                    .style(Component::DimText)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            "(Empty file)",
                            theme
                                .style(Component::DimText)
                                .add_modifier(Modifier::ITALIC),
                        )));
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                    lines.push(Line::from(Span::styled(
                        error.to_string(),
                        theme.style(Component::ErrorText),
                    )));
                }
                _ => {
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        theme.style(Component::ErrorText),
                    )));
                }
            }
        }

        lines
    }
}

fn extract_view_info(result: &Option<ToolResult>) -> String {
    match result {
        Some(ToolResult::FileContent(file_content)) => {
            let line_count = file_content.content.lines().count();
            format!("{line_count} lines")
        }
        Some(ToolResult::Error(_)) => "error".to_string(),
        _ => "pending".to_string(),
    }
}

fn format_line_range(offset: Option<u64>, limit: Option<u64>) -> String {
    let start = offset.map_or(1, |o| o + 1);
    match limit {
        Some(l) if l > 0 => format!("{}-{}", start, start + l - 1),
        _ => format!("{start}-EOF"),
    }
}
