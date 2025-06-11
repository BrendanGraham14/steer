use crate::app::conversation::{AssistantContent, Message, Role, ThoughtContent, ToolResult, UserContent};
use tools::tools::bash::{BASH_TOOL_NAME, BashParams};
use tools::tools::edit::{EDIT_TOOL_NAME, EditParams};
use tools::tools::replace::{REPLACE_TOOL_NAME, ReplaceParams};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use similar::{ChangeTag, TextDiff};
use textwrap;
use tracing::debug;

pub fn format_message(
    message: &Message,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut formatted_lines = Vec::new();

    // Handle each message variant differently
    match message {
        Message::User { content, .. } => {
            // Add role header
            formatted_lines.push(Line::from(Span::styled(
                "User:",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));

            // Format each content block
            for user_content in content {
                let block_lines = match user_content {
                    UserContent::Text { text } => format_text_block(text, terminal_width),
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => format_command_execution_block(command, stdout, stderr, *exit_code, terminal_width),
                };
                formatted_lines.extend(block_lines);
                formatted_lines.push(Line::raw(""));
            }
        }
        Message::Assistant { content, .. } => {
            // Add role header
            formatted_lines.push(Line::from(Span::styled(
                "Assistant:",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )));

            // Format each content block
            for assistant_content in content {
                let block_lines = match assistant_content {
                    AssistantContent::Text { text } => format_text_block(text, terminal_width),
                    AssistantContent::ToolCall { tool_call } => format_tool_call_block(tool_call, terminal_width),
                    AssistantContent::Thought { thought } => format_thought_block(&thought.display_text(), terminal_width),
                };
                formatted_lines.extend(block_lines);
                formatted_lines.push(Line::raw(""));
            }
        }
        Message::Tool { tool_use_id, result, .. } => {
            // Format tool result
            formatted_lines.extend(format_tool_result_for_message(tool_use_id, result, terminal_width));
        }
    }

    // Log for debugging
    debug!(
        target: "message_formatter",
        "Formatted message with role {:?}",
        message.role()
    );

    formatted_lines
}

/// Formats a text block with markdown rendering.
fn format_text_block(
    content: &str,
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
    tool_call: &tools::ToolCall,
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
                    // Format as replacement using diff
                    lines.push(Line::from(Span::styled(
                        format!(
                            "-> Calling Tool: edit (Applying diff to {})",
                            params.file_path
                        ),
                        Style::default().fg(Color::Cyan),
                    )));

                    let diff = TextDiff::from_lines(&params.old_string, &params.new_string);

                    for change in diff.iter_all_changes() {
                        let (sign, style) = match change.tag() {
                            ChangeTag::Delete => ("-", Style::default().fg(Color::Red)),
                            ChangeTag::Insert => ("+", Style::default().fg(Color::Green)),
                            ChangeTag::Equal => (" ", Style::default().fg(Color::DarkGray)),
                        };

                        let content = change.value_ref().trim_end_matches('\n'); // Trim trailing newline if present
                        for line_part in content.lines() {
                            // Potentially wrap long diff lines
                            for wrapped_line in
                                textwrap::wrap(line_part, wrap_width.saturating_sub(2))
                            // Subtract 2 for "+ " / "- "
                            {
                                lines.push(Line::from(Span::styled(
                                    format!("{} {}", sign, wrapped_line),
                                    style,
                                )));
                            }
                            // Handle empty lines within the change
                            if line_part.is_empty() {
                                lines.push(Line::from(Span::styled(
                                    sign.to_string(), // Show just the sign for empty lines
                                    style,
                                )));
                            }
                        }
                        // Ensure we handle the case where the original line was empty but wrapped_line might skip it
                        if content.is_empty() && change.tag() != ChangeTag::Equal {
                            // Only show sign for empty add/delete
                            lines.push(Line::from(Span::styled(sign.to_string(), style)));
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
                    format!("+++ {} (Full New Content)", params.file_path), // Clarify it's the full content
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
        BASH_TOOL_NAME => {
            if let Ok(params) = serde_json::from_value::<BashParams>(tool_call.parameters.clone()) {
                // Format bash command as a code block
                lines.push(Line::from(Span::styled(
                    "-> Calling Tool: bash",
                    Style::default().fg(Color::Cyan),
                )));
                lines.push(Line::from(Span::styled(
                    "```bash",
                    Style::default().fg(Color::DarkGray),
                )));

                // Format the command with proper wrapping
                for line in params.command.lines() {
                    for wrapped_line in textwrap::wrap(line, wrap_width.saturating_sub(2)) {
                        lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            Style::default().fg(Color::White),
                        )));
                    }
                }

                lines.push(Line::from(Span::styled(
                    "```",
                    Style::default().fg(Color::DarkGray),
                )));

                // Add timeout info if specified
                if let Some(timeout) = params.timeout {
                    lines.push(Line::from(Span::styled(
                        format!("   Timeout: {}ms", timeout),
                        Style::default().fg(Color::DarkGray),
                    )));
                }

                Some(()) // Indicate successful formatting
            } else {
                None // Deserialization failed, fallback to JSON
            }
        }
        _ => None,
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
                    "   Parameters: (Failed to format JSON)".to_string(),
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

/// Formats a tool result message for display.
fn format_tool_result_for_message(
    tool_use_id: &str,
    result: &ToolResult,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    
    match result {
        ToolResult::Success { output } => {
            lines.push(Line::from(Span::styled(
                format!("<- Tool Result for {}", tool_use_id),
                Style::default().fg(Color::Magenta),
            )));
            lines.extend(format_tool_preview(output, terminal_width));
        }
        ToolResult::Error { error } => {
            lines.push(Line::from(Span::styled(
                format!("<- Tool Error for {}", tool_use_id),
                Style::default().fg(Color::Red),
            )));
            lines.extend(format_tool_preview(error, terminal_width));
        }
    }
    
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

/// Formats a command response for display.
pub fn format_command_response(content: &str, terminal_width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Calculate wrap width from terminal width (accounting for margins)
    // Use a slightly different wrap width or style if desired for command responses
    let wrap_width = (terminal_width as usize).saturating_sub(6);
    let wrap_width = wrap_width.max(30);

    // Add a header or identifier
    lines.push(Line::from(Span::styled(
        "System/Command:",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD), // Distinct style
    )));

    for line in content.lines() {
        let wrapped_lines = textwrap::wrap(line, wrap_width);
        if wrapped_lines.is_empty() {
            // Preserve empty lines from the input
            lines.push(Line::raw("  ".to_string())); // Add indentation
        } else {
            for wrapped_line in wrapped_lines {
                // Indent the content and apply style
                lines.push(Line::from(Span::styled(
                    format!("  {}", wrapped_line),      // Add indentation
                    Style::default().fg(Color::Yellow), // Consistent style for content
                )));
            }
        }
    }
    lines
}

/// Formats a command execution block for display.
pub fn format_command_execution_block(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Calculate wrap width
    let wrap_width = (terminal_width as usize).saturating_sub(8);
    let wrap_width = wrap_width.max(40);

    // Show the command with a $ prompt
    lines.push(Line::from(vec![
        Span::styled(
            "$ ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(command.to_string(), Style::default().fg(Color::Cyan)),
    ]));

    // Show stdout if not empty
    if !stdout.trim().is_empty() {
        for line in stdout.lines() {
            let wrapped_lines = textwrap::wrap(line, wrap_width);
            if wrapped_lines.is_empty() {
                lines.push(Line::raw(""));
            } else {
                for wrapped_line in wrapped_lines {
                    lines.push(Line::from(Span::raw(wrapped_line.to_string())));
                }
            }
        }
    }

    // Show exit code and stderr if command failed
    if exit_code != 0 {
        lines.push(Line::from(Span::styled(
            format!("Exit code: {}", exit_code),
            Style::default().fg(Color::Red),
        )));

        if !stderr.trim().is_empty() {
            lines.push(Line::from(Span::styled(
                "Error output:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
            for line in stderr.lines() {
                let wrapped_lines = textwrap::wrap(line, wrap_width);
                if wrapped_lines.is_empty() {
                    lines.push(Line::raw(""));
                } else {
                    for wrapped_line in wrapped_lines {
                        lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            Style::default().fg(Color::Red),
                        )));
                    }
                }
            }
        }
    }

    lines
}

fn format_thought_block(text: &str, terminal_width: u16) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return vec![];
    }

    let thought_style = Style::default()
        .fg(Color::Gray)
        .add_modifier(Modifier::ITALIC);

    let mut content_lines = Vec::new();
    let prefix = "│ ";
    let indent_width = 2;

    // Calculate wrap width from terminal width
    let wrap_width = (terminal_width as usize).saturating_sub(4 + indent_width);
    let effective_wrap_width = wrap_width.max(20);

    // Use tui-markdown to render the markdown content
    let md_text = tui_markdown::from_str(text);

    // Process and format each line of the markdown-rendered thought
    for line in md_text.lines {
        let processed_spans: Vec<Span> = line
            .spans
            .into_iter()
            .map(|span| {
                let new_style = span
                    .style
                    .fg(thought_style.fg.unwrap())
                    .add_modifier(thought_style.add_modifier);
                Span::styled(span.content, new_style)
            })
            .collect();

        let line_text: String = processed_spans.iter().map(|s| s.content.as_ref()).collect();
        let heuristic_skip_wrap = line_text.len() <= effective_wrap_width
            || line_text.starts_with("    ")
            || line_text.starts_with('\t');

        if heuristic_skip_wrap {
            let owned_spans: Vec<Span<'static>> = processed_spans
                .into_iter()
                .map(|span| Span::styled(span.content.to_string(), span.style))
                .collect();
            let mut indented_spans = vec![Span::styled(prefix, thought_style)];
            indented_spans.extend(owned_spans);
            content_lines.push(Line::from(indented_spans));
        } else {
            let style_to_apply = processed_spans.first().map_or(thought_style, |s| s.style);
            let wrapped_text_lines = textwrap::wrap(&line_text, effective_wrap_width);

            for wrapped_segment in &wrapped_text_lines {
                content_lines.push(Line::from(vec![
                    Span::styled(prefix, thought_style),
                    Span::styled(wrapped_segment.to_string(), style_to_apply),
                ]));
            }
            if line_text.is_empty() && wrapped_text_lines.is_empty() {
                content_lines.push(Line::from(Span::styled(prefix, thought_style)));
            }
        }
    }

    // Determine the width for the box borders based on content
    let header_text = " Thought ";
    let min_width = header_text.len();
    let max_content_width = content_lines.iter().map(|l| l.width()).max().unwrap_or(0);
    // The width of the content lines includes the prefix ("│ "), so we subtract its width for the inner box calculation
    let inner_box_width = max_content_width
        .saturating_sub(indent_width)
        .max(min_width);

    // Assemble the final block with borders
    let mut final_lines = Vec::new();
    let header_padding = "─".repeat(inner_box_width.saturating_sub(header_text.len()));
    final_lines.push(Line::from(Span::styled(
        format!("┌─{}{}", header_text, header_padding),
        thought_style,
    )));

    final_lines.extend(content_lines);

    final_lines.push(Line::from(Span::styled(
        format!("└{}", "─".repeat(inner_box_width + 1)), // +1 for the corner
        thought_style,
    )));

    final_lines
}
