use super::ToolFormatter;
use crate::tui::theme::{Component, Theme};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use std::path::Path;
use steer_grpc::client_api::ToolResult;
use steer_tools::tools::read_file::ReadFileParams;

pub struct ReadFileFormatter;

impl ToolFormatter for ReadFileFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<ReadFileParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid read_file params",
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
        let info = extract_read_file_info(result);
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
        self.compact(params, result, wrap_width, theme)
    }
}

fn extract_read_file_info(result: &Option<ToolResult>) -> String {
    match result {
        Some(ToolResult::FileContent(file_content)) => {
            let line_count = file_content.content.lines().count();
            format!("{line_count} lines")
        }
        Some(ToolResult::Error(_)) => "error".to_string(),
        _ => "pending".to_string(),
    }
}
