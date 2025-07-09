use super::{ToolFormatter, helpers::*};
use crate::tui::theme::{Component, Theme};
use conductor_core::app::conversation::ToolResult;
use conductor_tools::tools::edit::{EditParams, multi_edit::MultiEditParams};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use similar::{Algorithm, ChangeTag, TextDiff};
use std::path::Path;

pub struct EditFormatter;

// Helper for detailed diffs
fn render_detailed_diff(
    old_string: &str,
    new_string: &str,
    wrap_width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .diff_lines(old_string, new_string);

    for change in diff.iter_all_changes() {
        let (prefix, style) = match change.tag() {
            ChangeTag::Delete => ("-", theme.style(Component::CodeDeletion)),
            ChangeTag::Insert => ("+", theme.style(Component::CodeAddition)),
            ChangeTag::Equal => (" ", theme.style(Component::DimText)),
        };

        let content = change.value().trim_end();

        // Only show a limited context for unchanged lines
        if change.tag() == ChangeTag::Equal {
            // Skip most unchanged lines, just show a few for context
            continue;
        }

        // Wrap long lines
        let wrapped_lines = textwrap::wrap(content, wrap_width.saturating_sub(2));
        if wrapped_lines.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(" ", style),
            ]));
        } else {
            for (i, wrapped_line) in wrapped_lines.iter().enumerate() {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(format!(" {wrapped_line}"), style),
                    ]));
                } else {
                    // Continuation lines
                    lines.push(Line::from(vec![
                        Span::styled("  ", style),
                        Span::styled(wrapped_line.to_string(), style),
                    ]));
                }
            }
        }
    }

    // Limit the number of diff lines shown
    const MAX_DIFF_LINES: usize = 20;
    if lines.len() > MAX_DIFF_LINES {
        let truncated_count = lines.len() - MAX_DIFF_LINES;
        lines.truncate(MAX_DIFF_LINES);
        lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
        lines.push(Line::from(Span::styled(
            format!("... ({truncated_count} more lines in diff)"),
            theme
                .style(Component::DimText)
                .add_modifier(Modifier::ITALIC),
        )));
    }

    lines
}

// Helper to show short context
fn show_short_context(text: &str, max_len: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_len {
        trimmed.to_string()
    } else {
        format!("{}...", &trimmed[..max_len.saturating_sub(3)])
    }
}

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
                        Span::raw(show_short_context(&edit.new_string, 60)),
                    ]));
                } else if edit.new_string.is_empty() {
                    // Deletion
                    lines.push(Line::from(vec![
                        Span::styled("- Delete: ", theme.style(Component::CodeDeletion)),
                        Span::raw(show_short_context(&edit.old_string, 60)),
                    ]));
                } else {
                    // Replacement
                    lines.push(Line::from(vec![
                        Span::styled("- ", theme.style(Component::CodeDeletion)),
                        Span::raw(show_short_context(&edit.old_string, 60)),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("+ ", theme.style(Component::CodeAddition)),
                        Span::raw(show_short_context(&edit.new_string, 60)),
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
                            error.to_string(),
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
                    show_short_context(&params.old_string, wrap_width.saturating_sub(2));
                for line in search_preview.lines() {
                    lines.push(Line::from(Span::raw(format!("  {line}"))));
                }
            }

            // Show detailed diff if we have room
            if !params.old_string.is_empty() || !params.new_string.is_empty() {
                lines.extend(render_detailed_diff(
                    &params.old_string,
                    &params.new_string,
                    wrap_width,
                    theme,
                ));
            }

            // Show result if available
            if let Some(ToolResult::Error(error)) = result {
                lines.push(separator_line(wrap_width, theme.style(Component::DimText)));
                lines.push(Line::from(Span::styled(
                    error.to_string(),
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
}
