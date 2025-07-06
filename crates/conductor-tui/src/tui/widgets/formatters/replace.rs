use super::ToolFormatter;
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::replace::ReplaceParams;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;

pub struct ReplaceFormatter;

impl ToolFormatter for ReplaceFormatter {
    fn compact(
        &self,
        params: &Value,
        _result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<ReplaceParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid replace params",
                styles::ERROR_TEXT,
            ))];
        };

        let line_count = params.content.lines().count();

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", params.file_path), Style::default()),
            Span::styled(format!("({line_count} lines)"), styles::ITALIC_GRAY),
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

        let Ok(params) = serde_json::from_value::<ReplaceParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid replace params",
                styles::ERROR_TEXT,
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("Replacing {}", params.file_path),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        if result.is_none() {
            // Show preview of new content
            lines.push(Line::from(Span::styled(
                format!("+++ {} (Full New Content)", params.file_path),
                styles::TOOL_SUCCESS,
            )));

            const MAX_PREVIEW_LINES: usize = 15;
            for (idx, line) in params.content.lines().enumerate() {
                if idx >= MAX_PREVIEW_LINES {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "... ({} more lines)",
                            params.content.lines().count() - MAX_PREVIEW_LINES
                        ),
                        styles::ITALIC_GRAY,
                    )));
                    break;
                }
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        format!("+ {wrapped_line}"),
                        styles::TOOL_SUCCESS,
                    )));
                }
            }
        }

        // Show error if result is an error
        if let Some(ToolResult::Error(error)) = result {
            lines.push(Line::from(Span::styled(
                error.to_string(),
                styles::ERROR_TEXT,
            )));
        }

        lines
    }
}
