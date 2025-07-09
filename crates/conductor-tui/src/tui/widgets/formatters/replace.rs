use super::ToolFormatter;
use crate::tui::theme::{Component, Theme};
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
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<ReplaceParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid replace params",
                theme.style(Component::ErrorText),
            ))];
        };

        let line_count = params.content.lines().count();

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", params.file_path), Style::default()),
            Span::styled(
                format!("({line_count} lines)"),
                theme
                    .style(Component::DimText)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));

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

        let Ok(params) = serde_json::from_value::<ReplaceParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid replace params",
                theme.style(Component::ErrorText),
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
                theme.style(Component::ToolSuccess),
            )));

            const MAX_PREVIEW_LINES: usize = 15;
            for (idx, line) in params.content.lines().enumerate() {
                if idx >= MAX_PREVIEW_LINES {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "... ({} more lines)",
                            params.content.lines().count() - MAX_PREVIEW_LINES
                        ),
                        theme
                            .style(Component::DimText)
                            .add_modifier(Modifier::ITALIC),
                    )));
                    break;
                }
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        format!("+ {wrapped_line}"),
                        theme.style(Component::ToolSuccess),
                    )));
                }
            }
        }

        // Show error if result is an error
        if let Some(ToolResult::Error(error)) = result {
            lines.push(Line::from(Span::styled(
                error.to_string(),
                theme.style(Component::ErrorText),
            )));
        }

        lines
    }
}
