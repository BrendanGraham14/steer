use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::astgrep::AstGrepParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct AstGrepFormatter;

impl ToolFormatter for AstGrepFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<AstGrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid astgrep params",
                styles::ERROR_TEXT,
            ))];
        };

        let path_display = params.path.as_deref().unwrap_or(".");

        let info = match result {
            Some(ToolResult::Search(search_result)) => {
                if search_result.matches.is_empty() {
                    "no matches".to_string()
                } else {
                    format!("{} matches", search_result.matches.len())
                }
            }
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "searching...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("pattern='{}' ", params.pattern), Style::default()),
            Span::styled(format!("path={} ", path_display), styles::DIM_TEXT),
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

        let Ok(params) = serde_json::from_value::<AstGrepParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid astgrep params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            "Search Parameters:",
            styles::TOOL_HEADER,
        )));
        lines.push(Line::from(Span::styled(
            format!("  Pattern: {}", params.pattern),
            Style::default(),
        )));

        if let Some(lang) = &params.lang {
            lines.push(Line::from(Span::styled(
                format!("  Lang: {}", lang),
                Style::default(),
            )));
        }
        if let Some(path) = &params.path {
            lines.push(Line::from(Span::styled(
                format!("  Path: {}", path),
                Style::default(),
            )));
        }
        if let Some(include) = &params.include {
            lines.push(Line::from(Span::styled(
                format!("  Include: {}", include),
                Style::default(),
            )));
        }
        if let Some(exclude) = &params.exclude {
            lines.push(Line::from(Span::styled(
                format!("  Exclude: {}", exclude),
                Style::default(),
            )));
        }

        // Render matches if result success
        if let Some(result) = result {
            match result {
                ToolResult::Search(search_result) => {
                    if !search_result.matches.is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_LINES: usize = 20;
                        let matches = &search_result.matches;

                        for search_match in matches.iter().take(MAX_LINES) {
                            let formatted = format!(
                                "{}:{}: {}",
                                search_match.file_path,
                                search_match.line_number,
                                search_match.line_content.trim()
                            );

                            for wrapped in textwrap::wrap(&formatted, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if matches.len() > MAX_LINES {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more matches)", matches.len() - MAX_LINES),
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
