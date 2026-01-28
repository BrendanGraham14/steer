use super::{
    ToolFormatter,
    helpers::{separator_line, tool_error_user_message},
};
use crate::tui::theme::{Component, Theme};
use ratatui::{
    style::Style,
    text::{Line, Span},
};
use serde_json::Value;
use steer_grpc::client_api::ToolResult;
use steer_tools::tools::todo::write::TodoWriteParams;
use steer_tools::tools::todo::{TodoItem, TodoPriority, TodoStatus};

/// Common function to format a list of todos grouped by status
fn format_todo_list(todos: &[TodoItem], theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if todos.is_empty() {
        lines.push(Line::from(Span::styled("No todos", theme.subtle_text())));
        return lines;
    }

    // Group by status
    let mut by_status: std::collections::HashMap<TodoStatus, Vec<&TodoItem>> =
        std::collections::HashMap::new();
    for todo in todos {
        by_status.entry(todo.status.clone()).or_default().push(todo);
    }

    // Display in order: in_progress, pending, completed
    let mut first_section = true;
    for status in &[
        TodoStatus::InProgress,
        TodoStatus::Pending,
        TodoStatus::Completed,
    ] {
        if let Some(todos) = by_status.get(status) {
            if !todos.is_empty() {
                // Add empty line between sections (but not before the first one)
                if !first_section {
                    lines.push(Line::from(""));
                }
                first_section = false;

                let status_style = match *status {
                    TodoStatus::InProgress => theme.style(Component::TodoInProgress),
                    TodoStatus::Pending => theme.style(Component::TodoPending),
                    TodoStatus::Completed => theme.style(Component::TodoCompleted),
                };

                lines.push(Line::from(Span::styled(
                    format!("{} ({}):", status, todos.len()),
                    status_style,
                )));

                // Sort todos by priority (high -> low)
                let mut sorted_todos = todos.clone();
                sorted_todos.sort_by_key(|todo| match todo.priority {
                    TodoPriority::High => 0,
                    TodoPriority::Medium => 1,
                    TodoPriority::Low => 2,
                });

                for todo in sorted_todos {
                    let priority_prefix = match todo.priority {
                        TodoPriority::High => "[H] ",
                        TodoPriority::Medium => "[M] ",
                        TodoPriority::Low => "[L] ",
                    };

                    let priority_style = match todo.priority {
                        TodoPriority::High => theme.style(Component::TodoHigh),
                        TodoPriority::Medium => theme.style(Component::TodoMedium),
                        TodoPriority::Low => theme.style(Component::TodoLow),
                    };

                    lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(priority_prefix, priority_style),
                        Span::raw(todo.content.clone()),
                    ]));
                }
            }
        }
    }

    lines
}

pub struct TodoReadFormatter;

impl ToolFormatter for TodoReadFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        self.detailed(params, result, wrap_width, theme)
    }

    fn detailed(
        &self,
        _params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(result) = result {
            match result {
                ToolResult::TodoRead(todo_list) => {
                    lines.push(Line::raw(""));
                    lines.push(separator_line(wrap_width, theme.dim_text()));

                    let mut todo_lines = format_todo_list(&todo_list.todos, theme);
                    lines.append(&mut todo_lines);
                }
                ToolResult::Error(error) => {
                    lines.push(Line::from(Span::styled(
                        tool_error_user_message(error).into_owned(),
                        theme.error_text(),
                    )));
                }
                _ => {
                    lines.push(Line::from(Span::styled(
                        "Unexpected result type",
                        theme.error_text(),
                    )));
                }
            }
        }

        lines
    }
}

pub struct TodoWriteFormatter;

impl ToolFormatter for TodoWriteFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        self.detailed(params, result, wrap_width, theme)
    }

    fn detailed(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let Ok(params) = serde_json::from_value::<TodoWriteParams>(params.clone()) else {
            return vec![Line::from(Span::styled(
                "Invalid todo write params",
                theme.error_text(),
            ))];
        };

        lines.push(Line::raw(""));
        lines.push(separator_line(wrap_width, theme.dim_text()));

        let mut todo_lines = format_todo_list(&params.todos, theme);
        lines.append(&mut todo_lines);

        if let Some(ToolResult::Error(error)) = result {
            lines.push(separator_line(wrap_width, theme.dim_text()));
            lines.push(Line::from(Span::styled(
                tool_error_user_message(error).into_owned(),
                theme.error_text(),
            )));
        }

        lines
    }
}
