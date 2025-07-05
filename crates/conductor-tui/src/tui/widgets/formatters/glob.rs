use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::glob::GlobParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct GlobFormatter;

impl ToolFormatter for GlobFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<GlobParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid glob params",
                styles::ERROR_TEXT,
            ))];
        };

        let path_display = params.path.as_deref().unwrap_or(".");

        let info = match result {
            Some(ToolResult::Glob(glob_result)) => {
                format!("{} matches", glob_result.matches.len())
            }
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "searching...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("pattern='{}' ", params.pattern), Style::default()),
            Span::styled(format!("path={path_display} "), styles::DIM_TEXT),
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

        let Ok(params) = serde_json::from_value::<GlobParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid glob params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("Pattern: {}", params.pattern),
            Style::default(),
        )));

        if let Some(path) = &params.path {
            lines.push(Line::from(Span::styled(
                format!("Path: {path}"),
                Style::default(),
            )));
        }

        // Show matches if we have results
        if let Some(result) = result {
            match result {
                ToolResult::Glob(glob_result) => {
                    if !glob_result.matches.is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_FILES: usize = 20;
                        let files = &glob_result.matches;

                        for file in files.iter().take(MAX_FILES) {
                            lines.push(Line::from(Span::raw(file.to_string())));
                        }

                        if files.len() > MAX_FILES {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more matches)", files.len() - MAX_FILES),
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
