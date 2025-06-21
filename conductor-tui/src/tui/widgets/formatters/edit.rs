use super::{ToolFormatter, helpers::*};
use crate::app::conversation::ToolResult;
use crate::tui::widgets::styles;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};
use tools::tools::edit::{EditParams, multi_edit::MultiEditParams};
use tracing::debug;

pub struct EditFormatter;

impl EditFormatter {
    fn format_single_edit(
        &self,
        params: EditParams,
        lines: &mut Vec<Line<'static>>,
        wrap_width: usize,
    ) {
        if params.old_string.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Creating {}", params.file_path),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));

            // Show preview of what will be created
            lines.push(Line::from(Span::styled(
                format!("+++ {}", params.file_path),
                styles::TOOL_SUCCESS,
            )));

            for line in params.new_string.lines() {
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        format!("+ {}", wrapped_line),
                        styles::TOOL_SUCCESS,
                    )));
                }
            }
        } else {
            lines.push(Line::from(Span::styled(
                format!("Applying diff to {}", params.file_path),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            // Show diff preview
            let diff = TextDiff::from_lines(&params.old_string, &params.new_string);

            for change in diff.iter_all_changes() {
                let (sign, style) = match change.tag() {
                    ChangeTag::Delete => ("-", styles::ERROR_TEXT),
                    ChangeTag::Insert => ("+", styles::TOOL_SUCCESS),
                    ChangeTag::Equal => (" ", styles::DIM_TEXT),
                };

                let content = change.value();

                if content.is_empty() || content == "\n" {
                    lines.push(Line::from(Span::styled(sign.to_string(), style)));
                } else {
                    let lines_to_process: Vec<&str> = if content.ends_with('\n') {
                        content[..content.len() - 1].lines().collect()
                    } else {
                        content.lines().collect()
                    };

                    for line in lines_to_process {
                        if line.is_empty() {
                            lines.push(Line::from(Span::styled(sign.to_string(), style)));
                        } else {
                            for wrapped_line in textwrap::wrap(line, wrap_width.saturating_sub(2)) {
                                lines.push(Line::from(Span::styled(
                                    format!("{} {}", sign, wrapped_line),
                                    style,
                                )));
                            }
                        }
                    }
                }
            }
        }
    }

    fn format_multi_edit(
        &self,
        params: MultiEditParams,
        lines: &mut Vec<Line<'static>>,
        wrap_width: usize,
    ) {
        lines.push(Line::from(Span::styled(
            format!(
                "Applying {} edits to {}",
                params.edits.len(),
                params.file_path
            ),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));

        for (i, edit) in params.edits.iter().enumerate() {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));
            lines.push(Line::from(Span::styled(
                format!("Edit {}/{}", i + 1, params.edits.len()),
                styles::ITALIC_GRAY,
            )));

            // Show a brief preview of each edit
            let old_preview = if edit.old_string.is_empty() {
                "(new content)".to_string()
            } else {
                truncate_middle(&edit.old_string.replace('\n', " "), 40)
            };
            let new_preview = truncate_middle(&edit.new_string.replace('\n', " "), 40);

            lines.push(Line::from(vec![
                Span::styled("- ", styles::ERROR_TEXT),
                Span::raw(old_preview),
            ]));
            lines.push(Line::from(vec![
                Span::styled("+ ", styles::TOOL_SUCCESS),
                Span::raw(new_preview),
            ]));
        }
    }
}

impl ToolFormatter for EditFormatter {
    fn compact(
        &self,
        params: &Value,
        result: &Option<ToolResult>,
        _wrap_width: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Try to parse as EditParams first, then MultiEditParams
        let (file_path, action, line_change) = match (
            serde_json::from_value::<EditParams>(params.clone()),
            serde_json::from_value::<MultiEditParams>(params.clone()),
        ) {
            (Ok(edit_params), _) => {
                let action = if edit_params.old_string.is_empty() {
                    "CREATE"
                } else {
                    "EDIT"
                };
                let line_change = edit_params.new_string.lines().count();
                (edit_params.file_path, action.to_string(), line_change)
            }
            (_, Ok(multi_params)) => {
                let total_lines: usize = multi_params
                    .edits
                    .iter()
                    .map(|edit| edit.new_string.lines().count())
                    .sum();
                let action = format!("MULTI-EDIT ({})", multi_params.edits.len());
                (multi_params.file_path, action, total_lines)
            }
            (Err(edit_error), Err(multi_error)) => {
                debug!("Error parsing params as edit: {:?}", edit_error);
                debug!("Error parsing params as multi edit: {:?}", multi_error);
                return vec![Line::from(Span::styled(
                    "Invalid edit params",
                    styles::ERROR_TEXT,
                ))];
            }
        };

        let info = if result.is_some() {
            format!("{} lines changed", line_change)
        } else {
            format!("{} lines", line_change)
        };

        lines.push(Line::from(vec![
            Span::styled(format!("{} ", action), Style::default().fg(Color::Yellow)),
            Span::styled(format!("file={} ", file_path), Style::default()),
            Span::styled(format!("({})", info), styles::ITALIC_GRAY),
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

        // Handle both EditParams and MultiEditParams
        if let Ok(params) = serde_json::from_value::<EditParams>(params.clone()) {
            self.format_single_edit(params, &mut lines, wrap_width);
        } else if let Ok(params) = serde_json::from_value::<MultiEditParams>(params.clone()) {
            self.format_multi_edit(params, &mut lines, wrap_width);
        } else {
            return vec![Line::from(Span::styled(
                "Invalid edit params",
                styles::ERROR_TEXT,
            ))];
        }

        // Show error if result is an error
        if let Some(ToolResult::Error { error }) = result {
            lines.push(separator_line(wrap_width, styles::DIM_TEXT));
            lines.push(Line::from(Span::styled(
                format!("Error: {}", error),
                styles::ERROR_TEXT,
            )));
        }

        lines
    }
}
