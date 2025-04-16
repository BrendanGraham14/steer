use crate::app::conversation::MessageContentBlock;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
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
        if !blocks.is_empty() {
            formatted_lines.push(Line::raw(""));
        }
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
        // Optionally add a blank line between different blocks for visual separation
        // formatted_lines.push(Line::raw(""));
    }

    // Remove trailing blank line if added
    // if formatted_lines.last().map_or(false, |l| l.spans.is_empty()) {
    //     formatted_lines.pop();
    // }

    formatted_lines
}

/// Formats a text block, handling markdown code highlighting.
fn format_text_block(
    content: &str,
    role: crate::app::Role,
    terminal_width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_block_content = String::new();
    let mut language = String::new();

    // Initialize syntax highlighting
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    let theme = &ts.themes["base16-ocean.dark"];

    // Calculate wrap width from terminal width (accounting for margins and borders)
    let wrap_width = (terminal_width as usize).saturating_sub(10);
    // Use a minimum width if terminal is very narrow
    let wrap_width = wrap_width.max(40);

    // Process the message line by line
    for line in content.lines() {
        // Check for code block delimiters
        if line.starts_with("```") {
            if in_code_block {
                // End of code block
                in_code_block = false;

                // Syntax highlight the code block
                if !code_block_content.is_empty() {
                    let syntax = if language.is_empty() {
                        ps.find_syntax_plain_text()
                    } else {
                        ps.find_syntax_by_token(&language)
                            .unwrap_or_else(|| ps.find_syntax_plain_text())
                    };

                    let mut highlighter = HighlightLines::new(syntax, theme);

                    for code_line in code_block_content.lines() {
                        let highlighted = highlighter
                            .highlight_line(code_line, &ps)
                            .unwrap_or_default();
                        let mut spans: Vec<Span> = Vec::new();

                        for (style, text) in highlighted {
                            let color = convert_syntect_style_to_color(&style);
                            spans.push(Span::styled(text.to_string(), Style::default().fg(color)));
                        }

                        lines.push(Line::from(spans));
                    }
                }

                code_block_content.clear();
                language.clear();
            } else {
                // Start of code block
                in_code_block = true;
                language = line.trim_start_matches("```").to_string();
            }
        } else if in_code_block {
            // Inside code block
            code_block_content.push_str(line);
            code_block_content.push('\n');
        } else {
            // Regular text - wrap and apply indentation if needed
            let wrapped_lines = textwrap::wrap(line, wrap_width);
            for wrapped_line in wrapped_lines {
                // Apply indentation if it's part of a User/Assistant message
                let text_span = Span::raw(wrapped_line.to_string());
                let line_content =
                    if matches!(role, crate::app::Role::User | crate::app::Role::Assistant) {
                        Line::from(vec![Span::raw("  "), text_span]) // Add indentation
                    } else {
                        Line::from(text_span)
                    };
                lines.push(line_content);
            }
        }
    }

    lines
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

    lines.push(Line::from(Span::styled(
        format!("-> Calling Tool: {}", tool_call.name),
        Style::default().fg(Color::Cyan), // Style for tool call indication
    )));
    // Pretty print JSON parameters
    match serde_json::to_string_pretty(&tool_call.parameters) {
        Ok(params_str) => {
            for line in params_str.lines() {
                // Create the indented line as an owned String
                let indented_line = format!("   {}", line);
                // Wrap long parameter lines
                let wrapped_lines = textwrap::wrap(&indented_line, wrap_width);
                for wrapped_line in wrapped_lines {
                    lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(), // Create an owned String
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

/// Convert a syntect style to a ratatui color
fn convert_syntect_style_to_color(style: &SyntectStyle) -> Color {
    if style.foreground.a == 0 {
        return Color::Reset;
    }

    Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)
}
