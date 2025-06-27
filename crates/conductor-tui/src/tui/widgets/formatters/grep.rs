use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::grep::GrepParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct GrepFormatter;

impl ToolFormatter for GrepFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<GrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid grep params",
                styles::ERROR_TEXT,
            ))];
        };

        let path_display = params
            .path
            .as_deref()
            .unwrap_or(".");
        let include_display = params
            .include
            .as_deref()
            .map(|i| format!(" ({})", i))
            .unwrap_or_default();

        let info = match result {
            Some(ToolResult::Search(search_result)) => {
                if search_result.matches.is_empty() {
                    "no matches".to_string()
                } else {
                    let unique_files: std::collections::HashSet<_> = search_result.matches
                        .iter()
                        .map(|m| m.file_path.as_str())
                        .collect();
                    format!("{} matches in {} files",
                        search_result.matches.len(),
                        unique_files.len()
                    )
                }
            }
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "searching...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("pattern='{}' ", params.pattern), Style::default()),
            Span::styled(
                format!("path={}{} ", path_display, include_display),
                styles::DIM_TEXT,
            ),
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

        let Ok(params) = serde_json::from_value::<GrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid grep params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("Pattern: {}", params.pattern),
            Style::default(),
        )));

        let path_display = params
            .path
            .as_deref()
            .unwrap_or("current directory");
        lines.push(Line::from(Span::styled(
            format!("Path: {}", path_display),
            Style::default(),
        )));

        if let Some(include) = &params.include {
            lines.push(Line::from(Span::styled(
                format!("  Include: {}", include),
                Style::default(),
            )));
        }

        // Show matches if we have results
        if let Some(result) = result {
            match result {
                ToolResult::Search(search_result) => {
                    if !search_result.matches.is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_MATCHES: usize = 15;
                        let matches = &search_result.matches;

                        for search_match in matches.iter().take(MAX_MATCHES) {
                            let formatted = format!("{}:{}: {}",
                                search_match.file_path,
                                search_match.line_number,
                                search_match.line_content.trim()
                            );
                            
                            for wrapped in textwrap::wrap(&formatted, wrap_width) {
                                lines.push(Line::from(Span::styled(
                                    wrapped.to_string(),
                                    Style::default(),
                                )));
                            }
                        }

                        if matches.len() > MAX_MATCHES {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more matches)", matches.len() - MAX_MATCHES),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            "No matches found",
                            styles::ITALIC_GRAY,
                        )));
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