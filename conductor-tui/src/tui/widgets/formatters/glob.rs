use super::{ToolFormatter, helpers::*};
use crate::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use tools::tools::glob::GlobParams;

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

        let info = if let Some(ToolResult::Success { output }) = result {
            let file_count = output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            format!("{} matches", file_count)
        } else if let Some(ToolResult::Error { .. }) = result {
            "failed".to_string()
        } else {
            "searching...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled("GLOB ", styles::DIM_TEXT),
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
                format!("Path: {}", path),
                Style::default(),
            )));
        }

        // Show matches if we have results
        if let Some(ToolResult::Success { output }) = result {
            if !output.trim().is_empty() {
                lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                const MAX_FILES: usize = 20;
                let files: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

                for file in files.iter().take(MAX_FILES) {
                    lines.push(Line::from(Span::raw(file.to_string())));
                }

                if files.len() > MAX_FILES {
                    lines.push(Line::from(Span::styled(
                        format!("... ({} more matches)", files.len() - MAX_FILES),
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        }

        lines
    }
}
