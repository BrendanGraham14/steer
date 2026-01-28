use super::{
    ToolFormatter,
    helpers::{separator_line, tool_error_user_message},
};
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::diff::{DiffMode, DiffWidget, diff_summary};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use std::path::Path;
use steer_grpc::client_api::ToolResult;
use steer_tools::tools::edit::{EditParams, multi_edit::MultiEditParams};

pub struct EditFormatter;

// Extract info for single edit
fn extract_edit_info(result: &Option<ToolResult>, old_string: &str, new_string: &str) -> String {
    match result {
        Some(ToolResult::Edit(_)) => {
            if old_string.is_empty() {
                "created".to_string()
            } else {
                format!(
                    "+{} -{}",
                    new_string.lines().count(),
                    old_string.lines().count()
                )
            }
        }
        Some(ToolResult::Error(_)) => "error".to_string(),
        _ => "pending".to_string(),
    }
}

impl ToolFormatter for EditFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
        theme: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Try parsing as MultiEditParams first
        if let Ok(params) = serde_json::from_value::<MultiEditParams>(params.clone()) {
            // Multi-edit formatting
            let file_name = Path::new(&params.file_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&params.file_path);

            let edit_count = params.edits.len();
            let info = match result {
                Some(ToolResult::Edit(_)) => {
                    if edit_count == 1 {
                        "1 edit applied".to_string()
                    } else {
                        format!("{edit_count} edits applied")
                    }
                }
                Some(ToolResult::Error(_)) => "error".to_string(),
                _ => format!("{edit_count} edits pending"),
            };

            lines.push(Line::from(vec![
                Span::styled(file_name.to_string(), Style::default()),
                Span::raw(" "),
                Span::styled(
                    format!("({info})"),
                    theme
                        .style(Component::DimText)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        } else if let Ok(params) = serde_json::from_value::<EditParams>(params.clone()) {
            // Single edit formatting
            let file_name = Path::new(&params.file_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&params.file_path);

            // Show a brief summary
            let mut spans = vec![Span::styled(file_name.to_string(), Style::default())];

            // Add info about the edit
            let info = extract_edit_info(result, &params.old_string, &params.new_string);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("({info})"),
                theme
                    .style(Component::DimText)
                    .add_modifier(Modifier::ITALIC),
            ));

            lines.push(Line::from(spans));
        } else {
            return vec![Line::from(Span::styled(
                "Invalid edit params",
                theme.style(Component::ErrorText),
            ))];
        }

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

        // Try parsing as MultiEditParams first
        if let Ok(params) = serde_json::from_value::<MultiEditParams>(params.clone()) {
            // Multi-edit formatting
            lines.push(Line::from(Span::styled(
                format!("Editing: {}", params.file_path),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            let edit_count = params.edits.len();
            lines.push(Line::from(Span::styled(
                format!("{edit_count} edits"),
                theme.style(Component::DimText),
            )));

            // Show each edit
            for (i, edit) in params.edits.iter().enumerate() {
                lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                lines.push(Line::from(Span::styled(
                    format!("Edit {}/{edit_count}:", i + 1),
                    theme.style(Component::ToolCallHeader),
                )));

                if edit.old_string.is_empty() {
                    // File creation or insertion
                    lines.push(Line::from(vec![
                        Span::styled("+ Insert: ", theme.style(Component::CodeAddition)),
                        Span::raw(diff_summary(&edit.old_string, &edit.new_string, 60).1),
                    ]));
                } else if edit.new_string.is_empty() {
                    // Deletion
                    lines.push(Line::from(vec![
                        Span::styled("- Delete: ", theme.style(Component::CodeDeletion)),
                        Span::raw(diff_summary(&edit.old_string, &edit.new_string, 60).0),
                    ]));
                } else {
                    // Replacement - show a mini diff
                    let (old_preview, new_preview) =
                        diff_summary(&edit.old_string, &edit.new_string, 60);
                    lines.push(Line::from(vec![
                        Span::styled("- ", theme.style(Component::CodeDeletion)),
                        Span::raw(old_preview),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("+ ", theme.style(Component::CodeAddition)),
                        Span::raw(new_preview),
                    ]));
                }
            }

            // Show result if available
            if let Some(result) = result {
                lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                match result {
                    ToolResult::Edit(_) => {
                        lines.push(Line::from(Span::styled(
                            format!("✓ All {edit_count} edits applied successfully"),
                            theme.style(Component::ToolSuccess),
                        )));
                    }
                    ToolResult::Error(error) => {
                        lines.push(Line::from(Span::styled(
                            tool_error_user_message(error).into_owned(),
                            theme.style(Component::ErrorText),
                        )));
                    }
                    _ => {}
                }
            }
        } else if let Ok(params) = serde_json::from_value::<EditParams>(params.clone()) {
            // Single edit formatting
            lines.push(Line::from(Span::styled(
                format!("Editing: {}", params.file_path),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            if result.is_none() && !params.old_string.is_empty() {
                // Show what we're looking for
                lines.push(Line::from(Span::styled(
                    "Searching for:",
                    theme.style(Component::DimText),
                )));
                let search_preview =
                    diff_summary(&params.old_string, "", wrap_width.saturating_sub(2)).0;
                for line in search_preview.lines() {
                    lines.push(Line::from(Span::raw(format!("  {line}"))));
                }
            }

            // Show detailed diff if we have room
            if !params.old_string.is_empty() || !params.new_string.is_empty() {
                let diff_widget = DiffWidget::new(&params.old_string, &params.new_string, theme)
                    .with_wrap_width(wrap_width)
                    .with_max_lines(20)
                    .with_mode(DiffMode::Split);
                lines.extend(diff_widget.lines());
            }

            // Show result if available
            if let Some(ToolResult::Error(error)) = result {
                lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                lines.push(Line::from(Span::styled(
                    tool_error_user_message(error).into_owned(),
                    theme.style(Component::ErrorText),
                )));
            }
        } else {
            return vec![Line::from(Span::styled(
                "Invalid edit params",
                theme.style(Component::ErrorText),
            ))];
        }

        lines
    }

    fn approval(&self, params: &Value, wrap_width: usize, theme: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Try parsing as MultiEditParams first
        if let Ok(params) = serde_json::from_value::<MultiEditParams>(params.clone()) {
            // Multi-edit formatting
            let file_name = Path::new(&params.file_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&params.file_path);

            lines.push(Line::from(vec![
                Span::styled("Edit ", theme.style(Component::DimText)),
                Span::styled(file_name.to_string(), Style::default()),
                Span::styled(
                    format!(" ({} changes)", params.edits.len()),
                    theme.style(Component::DimText),
                ),
            ]));

            // Show preview of first few edits
            const MAX_PREVIEW_EDITS: usize = 3;
            for (i, edit) in params.edits.iter().take(MAX_PREVIEW_EDITS).enumerate() {
                lines.push(Line::from(Span::styled(
                    format!("  Edit {}:", i + 1),
                    theme
                        .style(Component::DimText)
                        .add_modifier(Modifier::ITALIC),
                )));

                // Show a diff preview with context
                let diff_widget = DiffWidget::new(&edit.old_string, &edit.new_string, theme)
                    .with_wrap_width(wrap_width.saturating_sub(4)) // Account for indentation
                    .with_max_lines(6) // Limited preview per edit
                    .with_context_radius(1) // Minimal context for approval view
                    .with_mode(DiffMode::Split);

                // Indent the diff lines
                for line in diff_widget.lines() {
                    let mut indented_spans = vec![Span::raw("  ")];
                    indented_spans.extend(line.spans);
                    lines.push(Line::from(indented_spans));
                }
            }

            if params.edits.len() > MAX_PREVIEW_EDITS {
                lines.push(Line::from(Span::styled(
                    format!(
                        "  ... and {} more edits",
                        params.edits.len() - MAX_PREVIEW_EDITS
                    ),
                    theme
                        .style(Component::DimText)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        } else if let Ok(params) = serde_json::from_value::<EditParams>(params.clone()) {
            // Single edit formatting
            let file_name = Path::new(&params.file_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&params.file_path);

            lines.push(Line::from(vec![
                Span::styled("Edit ", theme.style(Component::DimText)),
                Span::styled(file_name.to_string(), Style::default()),
            ]));

            // Show brief diff preview with proper context
            let diff_widget = DiffWidget::new(&params.old_string, &params.new_string, theme)
                .with_wrap_width(wrap_width.saturating_sub(2))
                .with_max_lines(10) // Show up to 10 lines of diff
                .with_context_radius(2)
                .with_mode(DiffMode::Split); // Show 2 lines of context

            for line in diff_widget.lines() {
                let mut indented_spans = vec![Span::raw("  ")];
                indented_spans.extend(line.spans);
                lines.push(Line::from(indented_spans));
            }
        } else {
            return vec![Line::from(Span::styled(
                "Invalid edit params",
                theme.style(Component::ErrorText),
            ))];
        }

        lines
    }
}
