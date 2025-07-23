use super::{ToolFormatter, helpers::*};
use crate::tui::theme::Theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde_json::Value;
use steer_core::app::conversation::ToolResult;
use steer_tools::tools::todo::{TodoPriority, TodoStatus};

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
                    lines.push(Line::from(Span::styled("Todo List:", theme.text())));
                    lines.push(separator_line(wrap_width, theme.dim_text()));

                    if todo_list.todos.is_empty() {
                        lines.push(Line::from(Span::styled("No todos", theme.subtle_text())));
                    } else {
                        // Group by status
                        let mut by_status: std::collections::HashMap<TodoStatus, Vec<_>> =
                            std::collections::HashMap::new();
                        for todo in &todo_list.todos {
                            by_status.entry(todo.status.clone()).or_default().push(todo);
                        }

                        // Display in order: in_progress, pending, completed
                        for status in &[
                            TodoStatus::InProgress,
                            TodoStatus::Pending,
                            TodoStatus::Completed,
                        ] {
                            if let Some(todos) = by_status.get(status) {
                                if !todos.is_empty() {
                                    let status_color = match *status {
                                        TodoStatus::InProgress => Color::Yellow,
                                        TodoStatus::Pending => Color::Blue,
                                        TodoStatus::Completed => Color::Green,
                                    };

                                    lines.push(Line::from(Span::styled(
                                        format!("\n{} ({}):", status, todos.len()),
                                        Style::default().fg(status_color),
                                    )));

                                    for todo in todos {
                                        let priority_prefix = match todo.priority {
                                            TodoPriority::High => "[H] ",
                                            TodoPriority::Medium => "[M] ",
                                            TodoPriority::Low => "[L] ",
                                        };

                                        lines.push(Line::from(vec![
                                            Span::styled("  ", Style::default()),
                                            Span::styled(priority_prefix, theme.dim_text()),
                                            Span::raw(todo.content.clone()),
                                        ]));
                                    }
                                }
                            }
                        }
                    }
                }
                ToolResult::Error(error) => {
                    lines.push(Line::from(Span::styled(
                        error.to_string(),
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
