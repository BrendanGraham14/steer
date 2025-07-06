use super::{ToolFormatter, helpers::*};
use crate::tui::widgets::styles;
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::todo::write::TodoWriteParams;
use ratatui::{
    style::{Color, Style},
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
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<TodoWriteParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid todo write params",
                styles::ERROR_TEXT,
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
            Span::styled("TODO WRITE ", styles::DIM_TEXT),
            Span::styled(
                format!(
                    "({todo_count} items: {completed_count} completed, {in_progress_count} in progress, {pending_count} pending)"
                ),
                styles::ITALIC_GRAY,
            ),
        ]));

        lines
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<TodoWriteParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid todo write params",
                styles::ERROR_TEXT,
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
            let priority_color = match todo.priority {
                conductor_tools::tools::todo::TodoPriority::High => Color::Red,
                conductor_tools::tools::todo::TodoPriority::Medium => Color::Yellow,
                conductor_tools::tools::todo::TodoPriority::Low => Color::Green,
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{status_icon} "), Style::default()),
                Span::styled(
                    format!("[{}] ", format!("{:?}", todo.priority).to_uppercase()),
                    Style::default().fg(priority_color),
                ),
                Span::styled(todo.content.clone(), Style::default()),
            ]));
        }

        if let Some(ToolResult::Error(error)) = result {
            lines.push(separator_line(40, styles::DIM_TEXT));
            lines.push(Line::from(Span::styled(
                error.to_string(),
                styles::ERROR_TEXT,
            )));
        }

        lines
    }
}
