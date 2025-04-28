use crate::app::conversation::MessageContentBlock;
use crate::tools::edit::EDIT_TOOL_NAME;
use crate::tools::edit::EditParams;
use crate::tools::replace::REPLACE_TOOL_NAME;
use crate::tools::replace::ReplaceParams;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use textwrap;

/// Format a message (potentially with multiple content blocks) for display
pub fn format_message(
    blocks: &[MessageContentBlock],
    role: crate::app::Role,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    // Log for debugging
    crate::utils::logging::debug(
        "message_formatter",
        &format!(
            "Formatting message with {} blocks for role {:?}",
            blocks.len(),
            role
        ),
    );
    let mut formatted_lines = Vec::new();

    // Add Role Header (except for Tool/System which have specific formatting)
    let role_header = match role {
        crate::app::Role::User => Some(Line::from(Span::styled(
            "User:",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))),
        crate::app::Role::Assistant => Some(Line::from(Span::styled(
            "Assistant:",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ))),
        _ => None, // Tool and System messages have inherent headers in their formatters
    };

    if let Some(header) = role_header {
        formatted_lines.push(header);
        // Add a blank line after the header for separation, unless no blocks follow
        // if !blocks.is_empty() {
        //     formatted_lines.push(Line::raw(""));
        // }
    }

    for block in blocks {
        let block_lines = match block {
            MessageContentBlock::Text(content) => format_text_block(content, role, terminal_width), // Pass width
            MessageContentBlock::ToolCall(tool_call) => {
                format_tool_call_block(tool_call, terminal_width)
            } // Pass width
            MessageContentBlock::ToolResult {
                tool_use_id,
                result,
            } => format_tool_result_block(tool_use_id, result, terminal_width), // Pass width
        };
        formatted_lines.extend(block_lines);
        formatted_lines.push(Line::raw(""));
    }

    formatted_lines
}

/// Formats a text block with markdown rendering.
fn format_text_block(
    content: &str,
    _role: crate::app::Role, // Role no longer needed here
    terminal_width: u16,
) -> Vec<Line<'static>> {
    // Calculate wrap width from terminal width (accounting for List borders + padding)
    let wrap_width = (terminal_width as usize).saturating_sub(4);
    // Use a minimum width if terminal is very narrow
    let effective_wrap_width = wrap_width.max(20);

    // Check if content is empty
    if content.trim().is_empty() {
        return Vec::new();
    }

    // Use tui-markdown to render the markdown content
    let text = tui_markdown::from_str(content);

    // Convert to owned data with 'static lifetime and perform wrapping
    let mut final_lines: Vec<Line<'static>> = Vec::new();
    for line in text.lines {
        let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // Heuristic: Don't wrap lines that likely contain code or pre-formatted text
        // (starts with significant whitespace, or is short enough already)
        let heuristic_skip_wrap = line_text.len() <= effective_wrap_width
            || line_text.starts_with("    ") // Common code indentation
            || line_text.starts_with("	"); // Tabs

        if heuristic_skip_wrap {
            // Convert spans to 'static and add the line as is
            let owned_spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|span| Span::styled(span.content.to_string(), span.style))
                .collect();
            final_lines.push(Line::from(owned_spans));
        } else {
            // Wrap the line, applying the style of the first span to the whole wrapped segment
            let style_to_apply = line.spans.first().map_or(Style::default(), |s| s.style);
            let wrapped_text_lines = textwrap::wrap(&line_text, effective_wrap_width);

            for wrapped_segment in &wrapped_text_lines {
                // Iterate by reference
                final_lines.push(Line::styled(wrapped_segment.to_string(), style_to_apply));
            }
            // Preserve empty lines if textwrap resulted in nothing for an empty input line
            if line_text.is_empty() && wrapped_text_lines.is_empty() {
                final_lines.push(Line::raw(""));
            }
        }
    }

    // Return the processed lines. Wrapping is now done.
    final_lines
}

/// Formats a ToolCall block for display.
pub fn format_tool_call_block(
    tool_call: &crate::app::ToolCall,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Calculate wrap width from terminal width (accounting for margins and borders)
    let wrap_width = (terminal_width as usize).saturating_sub(10);
    // Use a minimum width if terminal is very narrow
    let wrap_width = wrap_width.max(40);

    let tool_name = tool_call.name.as_str();

    // Try to format specific tools as diffs
    let formatted_as_diff = match tool_name {
        EDIT_TOOL_NAME => {
            if let Ok(params) = serde_json::from_value::<EditParams>(tool_call.parameters.clone()) {
                if params.old_string.is_empty() {
                    // Format as file creation
                    lines.push(Line::from(Span::styled(
                        format!("-> Calling Tool: edit (Creating {})", params.file_path),
                        Style::default().fg(Color::Cyan),
                    )));
                    lines.push(Line::from(Span::styled(
                        format!("+++ {}", params.file_path),
                        Style::default().fg(Color::Green),
                    )));
                    for line in params.new_string.lines() {
                        for wrapped_line in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                format!("+ {}", wrapped_line),
                                Style::default().fg(Color::Green),
                            )));
                        }
                    }
                } else {
                    // Format as replacement
                    lines.push(Line::from(Span::styled(
                        format!("-> Calling Tool: edit (Modifying {})", params.file_path),
                        Style::default().fg(Color::Cyan),
                    )));
                    lines.push(Line::from(Span::styled(
                        "Replace:",
                        Style::default().fg(Color::DarkGray),
                    )));
                    for line in params.old_string.lines() {
                        for wrapped_line in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                format!("- {}", wrapped_line),
                                Style::default().fg(Color::Red),
                            )));
                        }
                    }
                    lines.push(Line::from(Span::styled(
                        "With:",
                        Style::default().fg(Color::DarkGray),
                    )));
                    for line in params.new_string.lines() {
                        for wrapped_line in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                format!("+ {}", wrapped_line),
                                Style::default().fg(Color::Green),
                            )));
                        }
                    }
                }
                Some(()) // Indicate successful diff formatting
            } else {
                None // Deserialization failed, fallback to JSON
            }
        }
        REPLACE_TOOL_NAME => {
            if let Ok(params) =
                serde_json::from_value::<ReplaceParams>(tool_call.parameters.clone())
            {
                // Format as file replacement (showing new content)
                lines.push(Line::from(Span::styled(
                    format!("-> Calling Tool: replace ({})", params.file_path),
                    Style::default().fg(Color::Cyan),
                )));
                lines.push(Line::from(Span::styled(
                    format!("+++ {} (New Content)", params.file_path),
                    Style::default().fg(Color::Green),
                )));
                for line in params.content.lines() {
                    for wrapped_line in textwrap::wrap(line, wrap_width) {
                        lines.push(Line::from(Span::styled(
                            format!("+ {}", wrapped_line),
                            Style::default().fg(Color::Green),
                        )));
                    }
                }
                Some(()) // Indicate successful diff formatting
            } else {
                None // Deserialization failed, fallback to JSON
            }
        }
        _ => None, // Not a tool we format as a diff
    };

    // Fallback to generic JSON formatting if diff formatting didn't happen
    if formatted_as_diff.is_none() {
        lines.push(Line::from(Span::styled(
            format!("-> Calling Tool: {}", tool_call.name),
            Style::default().fg(Color::Cyan),
        )));
        match serde_json::to_string_pretty(&tool_call.parameters) {
            Ok(params_str) => {
                for line in params_str.lines() {
                    let indented_line = format!("   {}", line);
                    let wrapped_lines = textwrap::wrap(&indented_line, wrap_width);
                    for wrapped_line in wrapped_lines {
                        lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }
            Err(_) => {
                lines.push(Line::from(Span::styled(
                    format!("   Parameters: (Failed to format JSON)"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }

    // Always add the Tool ID at the end
    lines.push(Line::from(Span::styled(
        format!("   (ID: {})", tool_call.id),
        Style::default().fg(Color::DarkGray),
    )));

    lines
}

/// Formats a ToolResult block for display.
pub fn format_tool_result_block(
    tool_use_id: &str,
    result: &str,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("<- Tool Result for {}", tool_use_id),
        Style::default().fg(Color::Magenta), // Style for tool result indication
    )));
    // Use the existing preview formatter for the result content
    lines.extend(format_tool_preview(result, terminal_width));
    lines
}

/// Format a tool result preview for display, potentially wrapping long lines
/// and applying some basic styling.
pub fn format_tool_preview(content: &str, terminal_width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Calculate wrap width from terminal width (accounting for margins and borders)
    let wrap_width = (terminal_width as usize).saturating_sub(10);
    // Use a minimum width if terminal is very narrow
    let wrap_width = wrap_width.max(40);

    for line in content.lines() {
        // Wrap long lines
        let wrapped_lines = textwrap::wrap(line, wrap_width);
        for wrapped_line in wrapped_lines {
            // Add a subtle style, e.g., dark gray, to distinguish tool output
            lines.push(Line::from(Span::styled(
                wrapped_line.to_string(), // Ensure we have an owned String
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    lines
}
