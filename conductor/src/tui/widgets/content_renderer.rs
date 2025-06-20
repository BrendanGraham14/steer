use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget},
};
use textwrap;

use crate::app::conversation::{AssistantContent, ToolResult, UserContent};
use tools::ToolCall;

use super::message_list::{MessageContent, ViewMode};
use super::styles;
use super::formatters;

/// Trait for rendering message content in different view modes
pub trait ContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer);
    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16;
}

/// Default implementation of ContentRenderer
pub struct DefaultContentRenderer;

impl DefaultContentRenderer {
    fn render_user_message(&self, blocks: &[UserContent], area: Rect, buf: &mut Buffer) {
        let mut content = Text::default();

        // Add role header
        content
            .lines
            .push(Line::from(Span::styled("User:", styles::ROLE_USER)));
        content.lines.push(Line::from(""));

        // Format each content block
        for (idx, block) in blocks.iter().enumerate() {
            match block {
                UserContent::Text { text } => {
                    let wrapped = self.wrap_text(text, area.width.saturating_sub(2));
                    content.extend(wrapped);
                }
                UserContent::CommandExecution {
                    command,
                    stdout,
                    stderr,
                    exit_code,
                } => {
                    let cmd_block = self.format_command_execution(
                        command,
                        stdout,
                        stderr,
                        *exit_code,
                        area.width.saturating_sub(2),
                    );
                    content.extend(cmd_block);
                }
                UserContent::AppCommand { command, response } => {
                    // For compact commands with actual summaries, render a separator first
                    if matches!(command, crate::app::conversation::AppCommandType::Compact) {
                        if let Some(crate::app::conversation::CommandResponse::Compact(
                            crate::app::conversation::CompactResult::Success(_),
                        )) = response
                        {
                            // Add a visual separator line
                            let separator = "━".repeat(area.width as usize);
                            content.lines.push(Line::from(Span::styled(
                                separator,
                                Style::default().fg(Color::DarkGray),
                            )));
                            content.lines.push(Line::from(Span::styled(
                                "Conversation Compacted",
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            )));
                            content.lines.push(Line::from(Span::styled(
                                "━".repeat(area.width as usize),
                                Style::default().fg(Color::DarkGray),
                            )));
                            content.lines.push(Line::from(""));
                        }
                    }

                    // Format app command and response
                    content.lines.push(Line::from(vec![
                        Span::styled("/", styles::COMMAND_PROMPT),
                        Span::styled(command.as_command_str(), styles::COMMAND_TEXT),
                    ]));

                    if let Some(resp) = response {
                        content.lines.push(Line::from(""));
                        let text = match resp {
                            crate::app::conversation::CommandResponse::Text(msg) => msg,
                            crate::app::conversation::CommandResponse::Compact(result) => match result {
                                crate::app::conversation::CompactResult::Success(summary) => summary,
                                crate::app::conversation::CompactResult::Cancelled => "Compact command cancelled.",
                                crate::app::conversation::CompactResult::InsufficientMessages => "Not enough messages to compact (minimum 10 required).",
                            },
                        };
                        let wrapped = self.wrap_text(text, area.width.saturating_sub(2));
                        content.extend(wrapped);
                    }
                }
            }
            // Only add spacing between blocks, not after the last one
            if idx + 1 < blocks.len() {
                content.lines.push(Line::from(""));
            }
        }

        let paragraph = Paragraph::new(content);
        paragraph.render(area, buf);
    }

    fn render_assistant_message(
        &self,
        blocks: &[AssistantContent],
        mode: ViewMode,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let mut y_offset = 0u16;

        // Add role header
        let header = vec![
            Line::from(Span::styled("Assistant:", styles::ROLE_ASSISTANT)),
            Line::from(""),
        ];
        let header_paragraph = Paragraph::new(header);
        let header_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 2.min(area.height),
        };
        if header_area.height > 0 {
            header_paragraph.render(header_area, buf);
            y_offset += header_area.height;
        }

        // Render each content block
        for (idx, block) in blocks.iter().enumerate() {
            // Stop rendering if we've exceeded the area
            if y_offset >= area.height {
                break;
            }

            let remaining_area = Rect {
                x: area.x,
                y: area.y + y_offset,
                width: area.width,
                height: area.height.saturating_sub(y_offset),
            };

            match block {
                AssistantContent::Text { text } => {
                    let wrapped = self.wrap_text(text, area.width.saturating_sub(2));
                    let line_count = wrapped.lines.len() as u16;
                    let text_area = Rect {
                        x: remaining_area.x,
                        y: remaining_area.y,
                        width: remaining_area.width,
                        height: line_count.min(remaining_area.height),
                    };
                    let paragraph = Paragraph::new(wrapped);
                    paragraph.render(text_area, buf);
                    y_offset += line_count;
                }
                AssistantContent::ToolCall { tool_call: _ } => {
                    // Skip rendering here because the Tool message itself is rendered separately
                    continue;
                }
                AssistantContent::Thought { thought } => {
                    let thought_text = thought.display_text();
                    let formatted_thought =
                        self.format_thought(&thought_text, area.width.saturating_sub(2));

                    // Calculate height needed for the thought block
                    let thought_height = 2 + formatted_thought.lines.len() as u16; // 2 for borders

                    // Create the thought area
                    let thought_area = Rect {
                        x: remaining_area.x,
                        y: remaining_area.y,
                        width: remaining_area.width,
                        height: thought_height.min(remaining_area.height),
                    };

                    // Create a titled block for the thought
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_style(styles::THOUGHT_BOX)
                        .title(Line::from(Span::styled(
                            " Thought ",
                            styles::THOUGHT_HEADER,
                        )));

                    // Create paragraph with the thought content
                    let thought_paragraph = Paragraph::new(formatted_thought).block(block);
                    thought_paragraph.render(thought_area, buf);

                    y_offset += thought_height;
                }
            }

            // Add spacing between blocks (but not after the last one)
            if idx + 1 < blocks.len() {
                y_offset += 1;
            }

            // Stop if we've run out of space
            if y_offset >= area.height {
                break;
            }
        }
    }

    fn render_tool_message(
        &self,
        call: &ToolCall,
        result: &Option<ToolResult>,
        mode: ViewMode,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let wrap_width = area.width.saturating_sub(4) as usize; // Account for borders and padding

        // Get the formatter for this tool
        let formatter = formatters::get_formatter(&call.name);

        // Format the tool with integrated call/result handling
        let all_lines = match mode {
            ViewMode::Compact => formatter.compact(&call.parameters, result, wrap_width),
            ViewMode::Detailed => formatter.detailed(&call.parameters, result, wrap_width),
        };

        // Draw the box with all content
        self.draw_tool_box(call, result, all_lines, area, buf);
    }

    fn draw_tool_box(
        &self,
        call: &ToolCall,
        result: &Option<ToolResult>,
        content_lines: Vec<Line<'static>>,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let box_style = styles::TOOL_BOX;

        // Build title with status indicator first, then tool name
        let mut title_spans = vec![];

        // Add status indicator if there's a result
        if let Some(result) = result {
            let (indicator, style) = match result {
                ToolResult::Success { .. } => (" ✓ ", styles::TOOL_SUCCESS),
                ToolResult::Error { .. } => (" ✗ ", styles::TOOL_ERROR),
            };
            title_spans.push(Span::styled(indicator, style));
        } else {
            // Add space to align with completed tools
            title_spans.push(Span::raw("   "));
        }

        // Add tool name
        title_spans.push(Span::styled(format!("{} ", call.name), box_style));

        // Add tool ID
        title_spans.push(Span::styled(format!("[{}]", call.id), styles::TOOL_ID));

        let title = Line::from(title_spans);

        // Create the block with borders and title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(box_style)
            .title(title);

        // Create paragraph with the content
        let content = Text::from(content_lines);
        let paragraph = Paragraph::new(content).block(block);

        // Render the paragraph
        paragraph.render(area, buf);
    }
    
    fn format_command_execution(
        &self,
        command: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        width: u16,
    ) -> Text<'static> {
        let mut content = Text::default();
        let wrap_width = width.saturating_sub(2) as usize;

        // Command header
        content.lines.push(Line::from(Span::styled(
            format!("$ {}", command),
            styles::COMMAND_PROMPT,
        )));

        // Stdout content (if any)
        if !stdout.is_empty() {
            let stdout_lines: Vec<&str> = stdout.lines().take(20).collect(); // Limit to 20 lines
            for line in stdout_lines {
                if line.len() <= wrap_width {
                    content.lines.push(Line::from(Span::styled(
                        line.to_string(),
                        styles::COMMAND_TEXT,
                    )));
                } else {
                    let wrapped = textwrap::wrap(line, wrap_width);
                    for wrapped_line in wrapped {
                        content.lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            styles::COMMAND_TEXT,
                        )));
                    }
                }
            }
        }

        // Stderr content (if any and exit code != 0)
        if exit_code != 0 && !stderr.is_empty() {
            content.lines.push(Line::from("")); // Separator
            content
                .lines
                .push(Line::from(Span::styled("stderr:", styles::ERROR_BOLD)));

            let stderr_lines: Vec<&str> = stderr.lines().take(10).collect(); // Limit to 10 lines
            for line in stderr_lines {
                if line.len() <= wrap_width {
                    content.lines.push(Line::from(Span::styled(
                        line.to_string(),
                        styles::ERROR_TEXT,
                    )));
                } else {
                    let wrapped = textwrap::wrap(line, wrap_width);
                    for wrapped_line in wrapped {
                        content.lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            }
        }

        // Exit code (only if non-zero)
        if exit_code != 0 {
            content.lines.push(Line::from("")); // Separator
            content.lines.push(Line::from(Span::styled(
                format!("Exit code: {}", exit_code),
                styles::ERROR_BOLD,
            )));
        }

        content
    }

    fn format_thought(&self, text: &str, width: u16) -> Text<'static> {
        let mut content = Text::default();

        for line in text.lines() {
            if line.len() <= width as usize {
                content.lines.push(Line::from(Span::styled(
                    line.to_string(),
                    styles::THOUGHT_TEXT,
                )));
            } else {
                let wrapped = textwrap::wrap(line, width as usize);
                for wrapped_line in wrapped {
                    content.lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        styles::THOUGHT_TEXT,
                    )));
                }
            }
        }

        content
    }

    fn wrap_text(&self, text: &str, width: u16) -> Text<'static> {
        let mut wrapped_text = Text::default();
        for line in text.lines() {
            if line.len() <= width as usize {
                wrapped_text.lines.push(Line::from(line.to_string()));
            } else {
                let wrapped = textwrap::wrap(line, width as usize);
                for wrapped_line in wrapped {
                    wrapped_text
                        .lines
                        .push(Line::from(wrapped_line.to_string()));
                }
            }
        }
        wrapped_text
    }
}

impl ContentRenderer for DefaultContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer) {
        match content {
            MessageContent::User { blocks, .. } => self.render_user_message(blocks, area, buf),
            MessageContent::Assistant { blocks, .. } => {
                self.render_assistant_message(blocks, mode, area, buf)
            }
            MessageContent::Tool { call, result, .. } => {
                self.render_tool_message(call, result, mode, area, buf)
            }
        }
    }

    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16 {
        let wrap_width = width.saturating_sub(2) as usize;

        match content {
            MessageContent::User { blocks, .. } => {
                let mut height = 2; // Role header

                for (idx, block) in blocks.iter().enumerate() {
                    match block {
                        UserContent::Text { text } => {
                            height += text
                                .lines()
                                .map(|line| {
                                    if line.len() <= wrap_width {
                                        1
                                    } else {
                                        textwrap::wrap(line, wrap_width).len() as u16
                                    }
                                })
                                .sum::<u16>();
                        }
                        UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        } => {
                            height += 1; // Command line
                            if !stdout.is_empty() {
                                height += stdout
                                    .lines()
                                    .take(20)
                                    .map(|line| {
                                        if line.len() <= wrap_width {
                                            1
                                        } else {
                                            textwrap::wrap(line, wrap_width).len() as u16
                                        }
                                    })
                                    .sum::<u16>();
                            }
                            if *exit_code != 0 && !stderr.is_empty() {
                                height += 2; // Separator and "stderr:" label
                                height += stderr
                                    .lines()
                                    .take(10)
                                    .map(|line| {
                                        if line.len() <= wrap_width {
                                            1
                                        } else {
                                            textwrap::wrap(line, wrap_width).len() as u16
                                        }
                                    })
                                    .sum::<u16>();
                            }
                            if *exit_code != 0 {
                                height += 2; // Separator and exit code
                            }
                        }
                        UserContent::AppCommand { command, response } => {
                            // Compact command separator
                            if matches!(command, crate::app::conversation::AppCommandType::Compact) {
                                if let Some(crate::app::conversation::CommandResponse::Compact(
                                    crate::app::conversation::CompactResult::Success(_),
                                )) = response
                                {
                                    height += 4; // Separator lines + "Conversation Compacted" + spacing
                                }
                            }

                            height += 1; // Command line
                            if let Some(resp) = response {
                                height += 1; // Empty line
                                let text = match resp {
                                    crate::app::conversation::CommandResponse::Text(msg) => msg,
                                    crate::app::conversation::CommandResponse::Compact(result) => match result {
                                        crate::app::conversation::CompactResult::Success(summary) => summary,
                                        crate::app::conversation::CompactResult::Cancelled => "Compact command cancelled.",
                                        crate::app::conversation::CompactResult::InsufficientMessages => "Not enough messages to compact (minimum 10 required).",
                                    },
                                };
                                height += text
                                    .lines()
                                    .map(|line| {
                                        if line.len() <= wrap_width {
                                            1
                                        } else {
                                            textwrap::wrap(line, wrap_width).len() as u16
                                        }
                                    })
                                    .sum::<u16>();
                            }
                        }
                    }
                    // Spacing between blocks
                    if idx + 1 < blocks.len() {
                        height += 1;
                    }
                }

                height
            }
            MessageContent::Assistant { blocks, .. } => {
                let mut height = 2; // Role header

                for (idx, block) in blocks.iter().enumerate() {
                    match block {
                        AssistantContent::Text { text } => {
                            height += text
                                .lines()
                                .map(|line| {
                                    if line.len() <= wrap_width {
                                        1
                                    } else {
                                        textwrap::wrap(line, wrap_width).len() as u16
                                    }
                                })
                                .sum::<u16>();
                        }
                        AssistantContent::ToolCall { .. } => {
                            // Tool calls are rendered as separate Tool messages
                        }
                        AssistantContent::Thought { thought } => {
                            let thought_text = thought.display_text();
                            height += 2; // Borders
                            height += thought_text
                                .lines()
                                .map(|line| {
                                    if line.len() <= wrap_width {
                                        1
                                    } else {
                                        textwrap::wrap(line, wrap_width).len() as u16
                                    }
                                })
                                .sum::<u16>();
                        }
                    }
                    // Spacing between blocks
                    if idx + 1 < blocks.len() {
                        height += 1;
                    }
                }

                height
            }
            MessageContent::Tool { call, result, .. } => {
                // Get the formatter for this tool
                let formatter = formatters::get_formatter(&call.name);
                
                // Format the content
                let lines = match mode {
                    ViewMode::Compact => formatter.compact(&call.parameters, result, wrap_width),
                    ViewMode::Detailed => formatter.detailed(&call.parameters, result, wrap_width),
                };
                
                // Height is lines + 2 for borders
                (lines.len() as u16) + 2
            }
        }
    }
}