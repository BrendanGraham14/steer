use super::{ToolFormatter, helpers::*};
use crate::tui::theme::Theme;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use serde_json::Value;
use steer_core::app::conversation::ToolResult;

pub struct TodoReadFormatter;

impl ToolFormatter for TodoReadFormatter {
    fn compact(
        &self,
        _params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let info = match result {
            Some(ToolResult::TodoRead(todo_list)) => {
                let pending = todo_list
                    .todos
                    .iter()
                    .filter(|t| t.status == "pending")
                    .count();
                let in_progress = todo_list
                    .todos
                    .iter()
                    .filter(|t| t.status == "in_progress")
                    .count();
                format!(
                    "{} todos ({} pending, {} in progress)",
                    todo_list.todos.len(),
                    pending,
                    in_progress
                )
            }
            Some(ToolResult::Error(_)) => "failed to read todos".to_string(),
            Some(_) => "unexpected result type".to_string(),
            None => "reading todos...".to_string(),
        };

        lines.push(Line::from(vec![
            Span::styled("TODO READ ", theme.dim_text()),
            Span::styled(format!("({info})"), theme.subtle_text()),
        ]));

        lines
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
                        let mut by_status: std::collections::HashMap<&str, Vec<_>> =
                            std::collections::HashMap::new();
                        for todo in &todo_list.todos {
                            by_status.entry(&todo.status).or_default().push(todo);
                        }

                        // Display in order: in_progress, pending, completed
                        for status in &["in_progress", "pending", "completed"] {
                            if let Some(todos) = by_status.get(status) {
                                if !todos.is_empty() {
                                    let status_color = match *status {
                                        "in_progress" => Color::Yellow,
                                        "pending" => Color::Blue,
                                        "completed" => Color::Green,
                                        _ => Color::Gray,
                                    };

                                    lines.push(Line::from(Span::styled(
                                        format!("\n{} ({}):", status.to_uppercase(), todos.len()),
                                        Style::default().fg(status_color),
                                    )));

                                    for todo in todos {
                                        let priority_prefix = match todo.priority.as_str() {
                                            "high" => "[H] ",
                                            "low" => "[L] ",
                                            _ => "",
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
