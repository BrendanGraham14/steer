use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::ls::LsParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

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

        let info = match result {
            Some(ToolResult::FileList(file_list)) => {
                format!("{} files", file_list.entries.len())
            }
            Some(ToolResult::Error(_)) => "failed".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "listing...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled(format!("path={dir_name} "), Style::default()),
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
        if let Some(result) = result {
            match result {
                ToolResult::FileList(file_list) => {
                    if !file_list.entries.is_empty() {
                        lines.push(separator_line(wrap_width, styles::DIM_TEXT));

                        const MAX_FILES: usize = 20;
                        let entries = &file_list.entries;

                        for entry in entries.iter().take(MAX_FILES) {
                            let prefix = if entry.is_directory { "[DIR] " } else { "" };
                            let display = format!("{}{}", prefix, entry.path);
                            lines.push(Line::from(Span::raw(display)));
                        }

                        if entries.len() > MAX_FILES {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more files)", entries.len() - MAX_FILES),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            "No files found",
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
