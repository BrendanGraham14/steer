use super::{ToolFormatter, helpers::*};
use conductor_core::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use conductor_tools::tools::ls::LsParams;

pub struct LsFormatter;

impl ToolFormatter for LsFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<LsParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid ls params",
                styles::ERROR_TEXT,
            ))];
        };

        let dir_name = std::path::Path::new(&params.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&params.path);

        let info = if let Some(ToolResult::Success { output }) = result {
            let file_count = output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            format!("{} files", file_count)
        } else if let Some(ToolResult::Error { .. }) = result {
            "failed".to_string()
        } else {
            "listing...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled(format!("path={} ", dir_name), Style::default()),
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

        let Ok(params) = serde_json::from_value::<LsParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid ls params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("Directory: {}", params.path),
            Style::default(),
        )));

        if let Some(ignore) = &params.ignore {
            lines.push(Line::from(Span::styled(
                format!("Ignore patterns: {}", ignore.join(", ")),
                styles::DIM_TEXT,
            )));
        }

        // Show files if we have results
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
                        format!("... ({} more files)", files.len() - MAX_FILES),
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        }

        lines
    }
}
