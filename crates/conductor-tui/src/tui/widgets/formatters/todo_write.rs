use super::{ToolFormatter, helpers::*};
use crate::tui::theme::{Component, Theme};
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::todo::write::TodoWriteParams;
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;

pub struct TodoWriteFormatter;

impl ToolFormatter for TodoWriteFormatter {
    fn compact(
        &self,
        params: &Value,
        _result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<TodoWriteParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid todo write params",
                theme.error_text(),
            ))];
        };

        let todo_count = params.todos.len();
        let (completed_count, in_progress_count, pending_count) = params.todos.iter().fold(
            (0, 0, 0),
            |(completed, in_progress, pending), todo| match todo.status {
                conductor_tools::tools::todo::TodoStatus::Completed => {
                    (completed + 1, in_progress, pending)
                }
                conductor_tools::tools::todo::TodoStatus::InProgress => {
                    (completed, in_progress + 1, pending)
                }
                conductor_tools::tools::todo::TodoStatus::Pending => {
                    (completed, in_progress, pending + 1)
                }
            },
        );

        lines.push(Line::from(vec![
            Span::styled("TODO WRITE ", theme.dim_text()),
            Span::styled(
                format!(
                    "({todo_count} items: {completed_count} completed, {in_progress_count} in progress, {pending_count} pending)"
                ),
                theme.subtle_text(),
            ),
        ]));

        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<TodoWriteParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid todo write params",
                theme.error_text(),
            ))];
        };

        lines.push(Line::from(Span::styled(
            format!("Updating {} todo items:", params.todos.len()),
            Style::default(),
        )));

        for todo in &params.todos {
            let status_icon = match todo.status {
                conductor_tools::tools::todo::TodoStatus::Pending => "⏳",
                conductor_tools::tools::todo::TodoStatus::InProgress => "🔄",
                conductor_tools::tools::todo::TodoStatus::Completed => "✅",
            };
            let priority_style = match todo.priority {
                conductor_tools::tools::todo::TodoPriority::High => {
                    theme.style(Component::TodoHigh)
                }
                conductor_tools::tools::todo::TodoPriority::Medium => {
                    theme.style(Component::TodoMedium)
                }
                conductor_tools::tools::todo::TodoPriority::Low => theme.style(Component::TodoLow),
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{status_icon} "), Style::default()),
                Span::styled(
                    format!("[{}] ", format!("{:?}", todo.priority).to_uppercase()),
                    priority_style,
                ),
                Span::styled(todo.content.clone(), Style::default()),
            ]));
        }

        if let Some(ToolResult::Error(error)) = result {
            lines.push(separator_line(40, theme.dim_text()));
            lines.push(Line::from(Span::styled(
                error.to_string(),
                theme.error_text(),
            )));
        }

        lines
    }
}
