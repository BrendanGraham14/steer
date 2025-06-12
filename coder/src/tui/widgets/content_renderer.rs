use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Widget},
};
use similar::{ChangeTag, TextDiff};
use textwrap;

use crate::app::conversation::{AssistantContent, ToolResult, UserContent};
use crate::tools::dispatch_agent::DISPATCH_AGENT_TOOL_NAME;
use crate::tools::fetch::FETCH_TOOL_NAME;
use tools::tools::bash::BashParams;
use tools::tools::edit::EditParams;
use tools::tools::replace::ReplaceParams;
use tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
    REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME, VIEW_TOOL_NAME,
};
use tools::{ToolCall, tools::view::ViewParams};

use super::message_list::{MessageContent, ViewMode};
use super::styles;

/// Trait for rendering message content in different view modes
pub trait ContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer);
    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16;
}

/// Default implementation of ContentRenderer
pub struct DefaultContentRenderer;

impl DefaultContentRenderer {
    fn get_tool_icon(tool_name: &str) -> &'static str {
        match tool_name {
            BASH_TOOL_NAME => "💻",
            EDIT_TOOL_NAME => "✏️",
            REPLACE_TOOL_NAME => "📝",
            VIEW_TOOL_NAME => "📖",
            LS_TOOL_NAME | GLOB_TOOL_NAME | GREP_TOOL_NAME => "📁",
            TODO_WRITE_TOOL_NAME | TODO_READ_TOOL_NAME => "📋",
            DISPATCH_AGENT_TOOL_NAME => "🤖",
            FETCH_TOOL_NAME => "🌐",
            _ => "🔧",
        }
    }

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
            height: 2,
        };
        header_paragraph.render(header_area, buf);
        y_offset += 2;

        // Render each content block
        for (idx, block) in blocks.iter().enumerate() {
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
                    let formatted_thought = self.format_thought(&thought_text, area.width.saturating_sub(2));
                    
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
        let mut all_lines = Vec::new();
        let wrap_width = area.width.saturating_sub(4) as usize; // Account for borders and padding

        // Format the tool call part
        let tool_lines = self.format_tool_call_lines(call, wrap_width, mode);
        all_lines.extend(tool_lines);

        // Add result if available
        if let Some(result) = result {
            // Add separator
            all_lines.push(Line::from(Span::styled(
                "─".repeat(wrap_width.min(40)),
                styles::DIM_TEXT,
            )));

            // Format result
            let result_lines = match result {
                ToolResult::Success { output } => {
                    self.format_tool_result_lines(output, wrap_width, false, mode)
                }
                ToolResult::Error { error } => {
                    self.format_tool_result_lines(error, wrap_width, true, mode)
                }
            };
            all_lines.extend(result_lines);
        }

        // Draw the box with all content
        self.draw_tool_box(call, result, all_lines, area, buf);
    }

    fn format_tool_call_lines(
        &self,
        call: &ToolCall,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        match call.name.as_str() {
            EDIT_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<EditParams>(call.parameters.clone()) {
                    lines = self.format_edit_tool_lines(&params, wrap_width, mode);
                }
            }
            REPLACE_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<ReplaceParams>(call.parameters.clone())
                {
                    lines = self.format_replace_tool_lines(&params, wrap_width, mode);
                }
            }
            BASH_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<BashParams>(call.parameters.clone()) {
                    lines = self.format_bash_tool_lines(&params, wrap_width, mode);
                }
            }
            VIEW_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<ViewParams>(call.parameters.clone()) {
                    lines = self.format_view_tool_lines(&params, wrap_width, mode);
                }
            }
            _ => {
                lines = self.format_default_tool_lines(call, wrap_width, mode);
            }
        }

        lines
    }

    fn format_edit_tool_lines(
        &self,
        params: &EditParams,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            lines.push(Line::from(Span::styled(
                format!(
                    "Edit {}: {} lines changed",
                    params.file_path,
                    params.new_string.lines().count()
                ),
                Style::default().fg(Color::Yellow),
            )));
        } else {
            // Detailed mode
            if params.old_string.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("Creating {}", params.file_path),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )));
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

                let diff = TextDiff::from_lines(&params.old_string, &params.new_string);
                for change in diff.iter_all_changes() {
                    let (sign, style) = match change.tag() {
                        ChangeTag::Delete => ("-", styles::ERROR_TEXT),
                        ChangeTag::Insert => ("+", styles::TOOL_SUCCESS),
                        ChangeTag::Equal => (" ", styles::DIM_TEXT),
                    };

                    let content = change.value().trim_end_matches('\n');
                    for line_part in content.lines() {
                        for wrapped_line in textwrap::wrap(line_part, wrap_width.saturating_sub(2))
                        {
                            lines.push(Line::from(Span::styled(
                                format!("{} {}", sign, wrapped_line),
                                style,
                            )));
                        }
                        if line_part.is_empty() {
                            lines.push(Line::from(Span::styled(sign.to_string(), style)));
                        }
                    }
                    if content.is_empty() && change.tag() != ChangeTag::Equal {
                        lines.push(Line::from(Span::styled(sign.to_string(), style)));
                    }
                }
            }
        }

        lines
    }

    fn format_replace_tool_lines(
        &self,
        params: &ReplaceParams,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            lines.push(Line::from(Span::styled(
                format!("Replace {}", params.file_path),
                Style::default().fg(Color::Yellow),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("Replacing {}", params.file_path),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                format!("+++ {} (Full New Content)", params.file_path),
                styles::TOOL_SUCCESS,
            )));
            for line in params.content.lines() {
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        format!("+ {}", wrapped_line),
                        styles::TOOL_SUCCESS,
                    )));
                }
            }
        }

        lines
    }

    fn format_bash_tool_lines(
        &self,
        params: &BashParams,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            let cmd_preview = if params.command.len() > 50 {
                format!("{}...", &params.command[..47])
            } else {
                params.command.clone()
            };
            lines.push(Line::from(Span::styled(
                format!("$ {}", cmd_preview),
                styles::COMMAND_TEXT,
            )));
        } else {
            for line in params.command.lines() {
                for wrapped_line in textwrap::wrap(line, wrap_width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        Style::default().fg(Color::White),
                    )));
                }
            }
            if let Some(timeout) = params.timeout {
                lines.push(Line::from(Span::styled(
                    format!("Timeout: {}ms", timeout),
                    styles::DIM_TEXT,
                )));
            }
        }

        lines
    }

    fn format_view_tool_lines(
        &self,
        params: &ViewParams,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        vec![Line::from(Span::styled(
            format!("Read({})", params.file_path),
            styles::DIM_TEXT,
        ))]
    }

    fn format_default_tool_lines(
        &self,
        call: &ToolCall,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Detailed {
            // Fallback to JSON
            if let Ok(json) = serde_json::to_string_pretty(&call.parameters) {
                for line in json.lines() {
                    let wrapped_lines = textwrap::wrap(line, wrap_width);
                    for wrapped_line in wrapped_lines {
                        lines.push(Line::from(Span::styled(
                            wrapped_line.to_string(),
                            styles::DIM_TEXT,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_tool_result_lines(
        &self,
        output: &str,
        wrap_width: usize,
        is_error: bool,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            if output.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    if is_error {
                        "✗ Error (no details)"
                    } else {
                        "✓ Completed (no output)"
                    },
                    if is_error {
                        styles::TOOL_ERROR
                    } else {
                        styles::TOOL_SUCCESS
                    },
                )));
            } else {
                let first_line = output.lines().next().unwrap_or("");
                let line_count = output.lines().count();
                if is_error {
                    lines.push(Line::from(Span::styled(
                        format!("✗ Error: {}", first_line),
                        styles::TOOL_ERROR,
                    )));
                } else {
                    lines.push(Line::from(Span::styled(
                        format!("✓ {} lines output", line_count),
                        styles::TOOL_SUCCESS,
                    )));
                }
            }
        } else {
            // Detailed mode
            if output.trim().is_empty() {
                lines.push(Line::from(Span::styled("(No output)", styles::ITALIC_GRAY)));
            } else {
                const MAX_PREVIEW_LINES: usize = 20;
                let output_lines: Vec<&str> = output.lines().collect();
                let truncated = output_lines.len() > MAX_PREVIEW_LINES;

                for (_idx, line) in output_lines.iter().take(MAX_PREVIEW_LINES).enumerate() {
                    let wrapped_lines = textwrap::wrap(line, wrap_width);
                    if wrapped_lines.is_empty() {
                        lines.push(Line::raw(""));
                    } else {
                        for wrapped_line in wrapped_lines {
                            lines.push(Line::from(Span::styled(
                                wrapped_line.to_string(),
                                if is_error {
                                    styles::ERROR_TEXT
                                } else {
                                    Style::default()
                                },
                            )));
                        }
                    }
                }

                if truncated {
                    lines.push(Line::from(Span::styled(
                        format!(
                            "... ({} more lines)",
                            output_lines.len() - MAX_PREVIEW_LINES
                        ),
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        }

        lines
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
        let icon = Self::get_tool_icon(&call.name);

        // Build title with multiple styled parts
        let mut title_spans = vec![
            Span::styled(format!(" {} {} ", icon, call.name), box_style),
            Span::styled(format!("[{}]", call.id), styles::TOOL_ID),
        ];

        // Add result indicator if there's a result
        if let Some(result) = result {
            let (indicator, style) = match result {
                ToolResult::Success { .. } => (" → ✓", styles::TOOL_SUCCESS),
                ToolResult::Error { .. } => (" → ✗", styles::TOOL_ERROR),
            };
            title_spans.push(Span::styled(indicator, style));
        }

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

        // Exit code indicator (if non-zero)
        if exit_code != 0 {
            content.lines.push(Line::from(Span::styled(
                format!("Exit code: {}", exit_code),
                styles::ERROR_TEXT,
            )));
        }

        content
    }

    fn format_thought(&self, text: &str, width: u16) -> Text<'static> {
        let mut lines = Vec::new();
        let wrap_width = width.saturating_sub(4) as usize;

        // Create content lines with proper wrapping
        for line in text.lines() {
            if line.is_empty() {
                lines.push(Line::raw(""));
            } else {
                for wrapped in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        wrapped.to_string(),
                        styles::THOUGHT_TEXT,
                    )));
                }
            }
        }

        // Return just the content - the box will be added when rendering
        Text::from(lines)
    }

    fn wrap_text(&self, text: &str, width: u16) -> Text<'static> {
        let mut content = Text::default();

        // Use tui-markdown for rich text formatting
        let md_text = tui_markdown::from_str(text);

        for line in md_text.lines {
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

            if line_text.len() <= width as usize || line_text.starts_with("    ") {
                // Don't wrap code or short lines
                let owned_spans: Vec<Span<'static>> = line
                    .spans
                    .into_iter()
                    .map(|span| Span::styled(span.content.to_string(), span.style))
                    .collect();
                content.lines.push(Line::from(owned_spans));
            } else {
                // Wrap long lines
                let wrapped = textwrap::wrap(&line_text, width as usize);
                for wrapped_line in wrapped {
                    content.lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        line.spans.first().map_or(Style::default(), |s| s.style),
                    )));
                }
            }
        }

        content
    }

    fn render_system_message(&self, text: &str, area: Rect, buf: &mut Buffer) {
        // Create wrapped text
        let wrapped_text = self.wrap_text(text, area.width.saturating_sub(2));

        // Create the block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(styles::ROLE_SYSTEM)
            .title(Line::from(Span::styled(
                " ℹ System Message ",
                styles::ROLE_SYSTEM,
            )));

        // Create paragraph with the content
        let paragraph = Paragraph::new(wrapped_text).block(block);

        // Render the paragraph
        paragraph.render(area, buf);
    }



    fn render_command_execution_message(
        &self,
        cmd: &str,
        stdout: &str,
        stderr: &str,
        exit_code: i32,
        mode: ViewMode,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let mut lines = Vec::new();
        let wrap_width = area.width.saturating_sub(4) as usize;

        // Command line
        lines.push(Line::from(vec![
            Span::styled("$ ", styles::COMMAND_PROMPT),
            Span::styled(cmd.to_string(), styles::COMMAND_TEXT),
        ]));

        if mode == ViewMode::Compact {
            // Just show status
            if exit_code == 0 {
                lines.push(Line::from(Span::styled(
                    "✓ Completed successfully",
                    styles::TOOL_SUCCESS,
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("✗ Failed with exit code {}", exit_code),
                    styles::TOOL_ERROR,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                "─".repeat(wrap_width.min(cmd.len() + 2)),
                styles::DIM_TEXT,
            )));

            // Stdout
            if !stdout.trim().is_empty() {
                const MAX_OUTPUT_LINES: usize = 20;
                let stdout_lines: Vec<&str> = stdout.lines().collect();

                for (_idx, line) in stdout_lines.iter().take(MAX_OUTPUT_LINES).enumerate() {
                    for wrapped in textwrap::wrap(line, wrap_width) {
                        lines.push(Line::from(Span::raw(wrapped.to_string())));
                    }
                }

                if stdout_lines.len() > MAX_OUTPUT_LINES {
                    lines.push(Line::from(Span::styled(
                        format!("... ({} more lines)", stdout_lines.len() - MAX_OUTPUT_LINES),
                        styles::ITALIC_GRAY,
                    )));
                }
            }

            // Error output and exit code
            if exit_code != 0 {
                lines.push(Line::from(Span::styled(
                    format!("Exit code: {}", exit_code),
                    styles::ERROR_TEXT,
                )));

                if !stderr.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "Error output:",
                        styles::ERROR_BOLD,
                    )));

                    const MAX_ERROR_LINES: usize = 10;
                    let stderr_lines: Vec<&str> = stderr.lines().collect();

                    for line in stderr_lines.iter().take(MAX_ERROR_LINES) {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                wrapped.to_string(),
                                styles::ERROR_TEXT,
                            )));
                        }
                    }

                    if stderr_lines.len() > MAX_ERROR_LINES {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "... ({} more error lines)",
                                stderr_lines.len() - MAX_ERROR_LINES
                            ),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            } else if stdout.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    "(Command completed successfully with no output)",
                    styles::ITALIC_GRAY,
                )));
            }
        }

        // Create the block with appropriate styling
        let box_style = if exit_code == 0 {
            styles::COMMAND_SUCCESS_BOX
        } else {
            styles::COMMAND_ERROR_BOX
        };

        let icon = if exit_code == 0 { "✓" } else { "✗" };
        let title = Line::from(Span::styled(
            format!(" {} Command Execution ", icon),
            box_style,
        ));

        // Create the block
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(box_style)
            .title(title);

        // Create paragraph with the content
        let content = Text::from(lines);
        let paragraph = Paragraph::new(content).block(block);

        // Render the paragraph
        paragraph.render(area, buf);
    }




}

impl ContentRenderer for DefaultContentRenderer {
    fn render(&self, content: &MessageContent, mode: ViewMode, area: Rect, buf: &mut Buffer) {
        match content {
            MessageContent::User { blocks, .. } => {
                self.render_user_message(blocks, area, buf);
            }
            MessageContent::Assistant { blocks, .. } => {
                self.render_assistant_message(blocks, mode, area, buf);
            }
            MessageContent::Tool { call, result, .. } => {
                self.render_tool_message(call, result, mode, area, buf);
            }
            MessageContent::System { text, .. } => {
                self.render_system_message(text, area, buf);
            }
        }
    }

    fn calculate_height(&self, content: &MessageContent, mode: ViewMode, width: u16) -> u16 {
        match content {
            MessageContent::User { blocks, .. } => {
                let mut height = 3; // Role header + spacing
                for block in blocks {
                    match block {
                        UserContent::Text { text } => {
                            // Use textwrap for accurate line counting
                            let lines = textwrap::wrap(text, width.saturating_sub(2) as usize);
                            height += lines.len() as u16;
                        }
                        UserContent::CommandExecution {
                            stdout,
                            stderr,
                            exit_code,
                            ..
                        } => {
                            if mode != ViewMode::Compact {
                                height += stdout.lines().count().min(20) as u16;
                                if *exit_code != 0 && !stderr.is_empty() {
                                    height += stderr.lines().count().min(10) as u16 + 2;
                                }
                            }
                        }
                    }
                }
                height
            }
            MessageContent::Assistant { blocks, .. } => {
                let mut height = 2; // Role header + one blank line after it
                for (idx, block) in blocks.iter().enumerate() {
                    match block {
                        AssistantContent::Text { text } => {
                            let lines = textwrap::wrap(text, width.saturating_sub(2) as usize);
                            height += lines.len() as u16;
                        }
                        AssistantContent::ToolCall { .. } => {
                            // Do not count height for tool call – separate Tool message shows it
                        }
                        AssistantContent::Thought { thought } => {
                            // Block borders (top + bottom) + content
                            height += 2;
                            let thought_text = thought.display_text();
                            let lines =
                                textwrap::wrap(&thought_text, width.saturating_sub(4) as usize);
                            height += lines.len() as u16;
                        }
                    }
                    // Add spacing between blocks (but not after the last one)
                    if idx + 1 < blocks.len() {
                        height += 1;
                    }
                }
                height
            }
            MessageContent::Tool { call, result, .. } => {
                // Box borders
                let mut height = 3;

                // Tool call content
                let call_lines =
                    self.format_tool_call_lines(call, width.saturating_sub(4) as usize, mode);
                height += call_lines.len() as u16;

                // Result if present
                if let Some(result) = result {
                    // height += 1; // Separator
                    let output = match result {
                        ToolResult::Success { output } => output,
                        ToolResult::Error { error } => error,
                    };
                    let result_lines = self.format_tool_result_lines(
                        output,
                        width.saturating_sub(4) as usize,
                        matches!(result, ToolResult::Error { .. }),
                        mode,
                    );
                    height += result_lines.len() as u16;
                }

                height
            }
            MessageContent::System { text, .. } => {
                // Box borders + content
                let mut height = 3;
                let lines = textwrap::wrap(text, width.saturating_sub(4) as usize);
                height += lines.len() as u16;
                height
            }
        }
    }
}
