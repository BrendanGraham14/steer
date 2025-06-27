use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde_json::Value;

pub struct TodoReadFormatter;

impl ToolFormatter for TodoReadFormatter {
    fn compact(
        &self,
        _params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let info = if let Some(ToolResult::Success { output }) = result {
            // Try to parse as JSON to show summary
            if let Ok(todos) = serde_json::from_str::<Vec<serde_json::Value>>(output) {
                let pending = todos
                    .iter()
                    .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("pending"))
                    .count();
                let in_progress = todos
                    .iter()
                    .filter(|t| t.get("status").and_then(|s| s.as_str()) == Some("in_progress"))
                    .count();
                format!(
                    "{} todos ({} pending, {} in progress)",
                    todos.len(),
                    pending,
                    in_progress
                )
            } else {
                "todo list".to_string()
            }
        } else if let Some(ToolResult::Error { .. }) = result {
            "failed to read todos".to_string()
        } else {
            "reading todos...".to_string()
        };

        lines.push(Line::from(vec![
            Span::styled("TODO READ ", styles::DIM_TEXT),
            Span::styled(format!("({})", info), styles::ITALIC_GRAY),
        ]));

        lines
    }

    fn detailed(
        &self,
        _params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(ToolResult::Success { output }) = result {
            lines.push(Line::from(Span::styled("Todo List:", styles::TOOL_HEADER)));

            if let Ok(todos) = serde_json::from_str::<Vec<serde_json::Value>>(output) {
                for todo in &todos {
                    if let (Some(content), Some(status), Some(priority)) = (
                        todo.get("content").and_then(|c| c.as_str()),
                        todo.get("status").and_then(|s| s.as_str()),
                        todo.get("priority").and_then(|p| p.as_str()),
                    ) {
                        let status_icon = match status {
                            "pending" => "⏳",
                            "in_progress" => "🔄",
                            "completed" => "✅",
                            _ => "❓",
                        };
                        let priority_color = match priority {
                            "high" => Color::Red,
                            "medium" => Color::Yellow,
                            "low" => Color::Green,
                            _ => Color::White,
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!("{} ", status_icon), Style::default()),
                            Span::styled(
                                format!("[{}] ", priority.to_uppercase()),
                                Style::default().fg(priority_color),
                            ),
                            Span::styled(content.to_string(), Style::default()),
                        ]));
                    }
                }
            } else {
                // Fallback to raw output
                const MAX_LINES: usize = 20;
                let (output_lines, truncated) = truncate_lines(output, MAX_LINES);

                for line in output_lines {
                    for wrapped in textwrap::wrap(line, wrap_width) {
                        lines.push(Line::from(Span::raw(wrapped.to_string())));
                    }
                }

                if truncated {
                    lines.push(Line::from(Span::styled(
                        format!("... ({} more lines)", output.lines().count() - MAX_LINES),
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        } else if let Some(ToolResult::Error { error }) = result {
            lines.push(Line::from(Span::styled(
                format!("Error reading todos: {}", error),
                styles::ERROR_TEXT,
            )));
        } else {
            lines.push(Line::from(Span::styled("Read todo list", styles::DIM_TEXT)));
        }

        lines
    }
}
