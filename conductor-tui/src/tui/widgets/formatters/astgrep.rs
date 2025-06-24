use super::{ToolFormatter, helpers::*};
use crate::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use tools::tools::astgrep::AstGrepParams;

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

        let info = if let Some(ToolResult::Success { output }) = result {
            let match_count = output.lines().count();
            if match_count == 0 {
                "no matches".to_string()
            } else {
                format!("{} matches", match_count)
            }
        } else if let Some(ToolResult::Error { .. }) = result {
            "failed".to_string()
        } else {
            "searching...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled("ASTGREP ", styles::DIM_TEXT),
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
        if let Some(ToolResult::Success { output }) = result {
            if !output.trim().is_empty() {
                lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                const MAX_LINES: usize = 20;
                let matches: Vec<&str> = output.lines().collect();
                for m in matches.iter().take(MAX_LINES) {
                    for wrapped in textwrap::wrap(m, wrap_width) {
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

        lines
    }
}
