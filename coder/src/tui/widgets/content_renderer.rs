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
use crate::tools::dispatch_agent::{DISPATCH_AGENT_TOOL_NAME, DispatchAgentParams};
use crate::tools::fetch::{FETCH_TOOL_NAME, FetchParams};
use tools::tools::bash::BashParams;
use tools::tools::edit::EditParams;
use tools::tools::glob::GlobParams;
use tools::tools::grep::GrepParams;
use tools::tools::ls::LsParams;
use tools::tools::replace::ReplaceParams;
use tools::tools::todo::write::TodoWriteParams;
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

        // Format the tool with integrated call/result handling
        let all_lines = self.format_tool_with_result(call, result, wrap_width, mode);

        // Draw the box with all content
        self.draw_tool_box(call, result, all_lines, area, buf);
    }

    fn format_tool_with_result(
        &self,
        call: &ToolCall,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        match call.name.as_str() {
            EDIT_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<EditParams>(call.parameters.clone()) {
                    self.format_edit_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            REPLACE_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<ReplaceParams>(call.parameters.clone())
                {
                    self.format_replace_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            BASH_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<BashParams>(call.parameters.clone()) {
                    self.format_bash_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            VIEW_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<ViewParams>(call.parameters.clone()) {
                    self.format_view_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            GREP_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<GrepParams>(call.parameters.clone()) {
                    self.format_grep_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            LS_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<LsParams>(call.parameters.clone()) {
                    self.format_ls_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            GLOB_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<GlobParams>(call.parameters.clone()) {
                    self.format_glob_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            TODO_READ_TOOL_NAME => self.format_todo_read_tool(result, wrap_width, mode),
            TODO_WRITE_TOOL_NAME => {
                if let Ok(params) =
                    serde_json::from_value::<TodoWriteParams>(call.parameters.clone())
                {
                    self.format_todo_write_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            DISPATCH_AGENT_TOOL_NAME => {
                if let Ok(params) =
                    serde_json::from_value::<DispatchAgentParams>(call.parameters.clone())
                {
                    self.format_dispatch_agent_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            FETCH_TOOL_NAME => {
                if let Ok(params) = serde_json::from_value::<FetchParams>(call.parameters.clone()) {
                    self.format_fetch_tool(&params, result, wrap_width, mode)
                } else {
                    self.format_default_tool(call, result, wrap_width, mode)
                }
            }
            _ => self.format_default_tool(call, result, wrap_width, mode),
        }
    }

    fn format_edit_tool(
        &self,
        params: &EditParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            let action = if params.old_string.is_empty() {
                "Create"
            } else {
                "Edit"
            };
            let line_change = params.new_string.lines().count();

            if result.is_some() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "{} {}: {} lines changed",
                        action, params.file_path, line_change
                    ),
                    Style::default().fg(Color::Yellow),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("{} {}", action, params.file_path),
                    Style::default().fg(Color::Yellow),
                )));
            }
        } else {
            // Detailed mode
            if params.old_string.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("Creating {}", params.file_path),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )));

                // Show preview of what will be created or what was created
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

                // Show diff preview (both before and after the edit)
                let diff = TextDiff::from_lines(&params.old_string, &params.new_string);

                for change in diff.iter_all_changes() {
                    let (sign, style) = match change.tag() {
                        ChangeTag::Delete => ("-", styles::ERROR_TEXT),
                        ChangeTag::Insert => ("+", styles::TOOL_SUCCESS),
                        ChangeTag::Equal => (" ", styles::DIM_TEXT),
                    };

                    // Get the change content, preserving empty lines
                    let content = change.value();

                    // Handle the content line by line
                    if content.is_empty() || content == "\n" {
                        // Empty line in diff
                        lines.push(Line::from(Span::styled(sign.to_string(), style)));
                    } else {
                        // Process each line in the change
                        let lines_to_process: Vec<&str> = if content.ends_with('\n') {
                            // Remove the trailing newline for processing
                            content[..content.len() - 1].lines().collect()
                        } else {
                            content.lines().collect()
                        };

                        for line in lines_to_process {
                            if line.is_empty() {
                                // Empty line
                                lines.push(Line::from(Span::styled(sign.to_string(), style)));
                            } else {
                                // Wrap long lines
                                for wrapped_line in
                                    textwrap::wrap(line, wrap_width.saturating_sub(2))
                                {
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

            // Show result status
            if let Some(result) = result {
                match result {
                    ToolResult::Success { .. } => {
                        // No need to show anything for success
                    }
                    ToolResult::Error { error } => {
                        lines.push(Line::from(Span::styled(
                            "─".repeat(wrap_width.min(40)),
                            styles::DIM_TEXT,
                        )));
                        lines.push(Line::from(Span::styled(
                            format!("Error: {}", error),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_bash_tool(
        &self,
        params: &BashParams,
        result: &Option<ToolResult>,
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

            if let Some(result) = result {
                match result {
                    ToolResult::Success { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("$ {}", cmd_preview),
                            styles::COMMAND_TEXT,
                        )));
                    }
                    ToolResult::Error { error } => {
                        // Extract exit code if available
                        let exit_code = error
                            .lines()
                            .find(|l| l.starts_with("Exit code:"))
                            .and_then(|l| l.split(": ").nth(1))
                            .unwrap_or("?");
                        lines.push(Line::from(Span::styled(
                            format!("$ {} (exit {})", cmd_preview, exit_code),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("$ {}", cmd_preview),
                    styles::COMMAND_TEXT,
                )));
            }
        } else {
            // Detailed mode - show full command
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

            // Show output if we have results
            if let Some(result) = result {
                lines.push(Line::from(Span::styled(
                    "─".repeat(wrap_width.min(40)),
                    styles::DIM_TEXT,
                )));

                match result {
                    ToolResult::Success { output } => {
                        if output.trim().is_empty() {
                            lines.push(Line::from(Span::styled(
                                "(Command completed successfully with no output)",
                                styles::ITALIC_GRAY,
                            )));
                        } else {
                            const MAX_OUTPUT_LINES: usize = 20;
                            let output_lines: Vec<&str> = output.lines().collect();

                            for line in output_lines.iter().take(MAX_OUTPUT_LINES) {
                                for wrapped in textwrap::wrap(line, wrap_width) {
                                    lines.push(Line::from(Span::raw(wrapped.to_string())));
                                }
                            }

                            if output_lines.len() > MAX_OUTPUT_LINES {
                                lines.push(Line::from(Span::styled(
                                    format!(
                                        "... ({} more lines)",
                                        output_lines.len() - MAX_OUTPUT_LINES
                                    ),
                                    styles::ITALIC_GRAY,
                                )));
                            }
                        }
                    }
                    ToolResult::Error { error } => {
                        // Show error output
                        for line in error.lines().take(10) {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::styled(
                                    wrapped.to_string(),
                                    styles::ERROR_TEXT,
                                )));
                            }
                        }
                    }
                }
            }
        }

        lines
    }

    fn format_grep_tool(
        &self,
        params: &GrepParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let path_display = params.path.as_deref().unwrap_or(".");
        let include_display = params
            .include
            .as_ref()
            .map(|i| format!(" ({})", i))
            .unwrap_or_default();

        if mode == ViewMode::Compact {
            // Compact mode - show search and result summary
            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let match_count = output.lines().count();
                        if match_count == 0 {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "Grep '{}' in {}{} - no matches",
                                    params.pattern, path_display, include_display
                                ),
                                styles::DIM_TEXT,
                            )));
                        } else {
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "Grep '{}' in {}{} - {} matches",
                                    params.pattern, path_display, include_display, match_count
                                ),
                                styles::DIM_TEXT,
                            )));
                        }
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Grep '{}' failed", params.pattern),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!(
                        "Grep '{}' in {}{}",
                        params.pattern, path_display, include_display
                    ),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                "Search Parameters:",
                styles::TOOL_HEADER,
            )));
            lines.push(Line::from(Span::styled(
                format!("  Pattern: {}", params.pattern),
                Style::default(),
            )));
            if let Some(path) = &params.path {
                lines.push(Line::from(Span::styled(
                    format!("  Path: {}", path),
                    Style::default(),
                )));
            }
            if let Some(include) = &params.include {
                lines.push(Line::from(Span::styled(
                    format!("  Include: {}", include),
                    Style::default(),
                )));
            }

            // Show matches if we have results
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_MATCHES: usize = 15;
                    let matches: Vec<&str> = output.lines().collect();

                    for line in matches.iter().take(MAX_MATCHES) {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::styled(
                                wrapped.to_string(),
                                Style::default(),
                            )));
                        }
                    }

                    if matches.len() > MAX_MATCHES {
                        lines.push(Line::from(Span::styled(
                            format!("... ({} more matches)", matches.len() - MAX_MATCHES),
                            styles::ITALIC_GRAY,
                        )));
                    }
                } else {
                    lines.push(Line::from(Span::styled(
                        "No matches found",
                        styles::ITALIC_GRAY,
                    )));
                }
            }
        }

        lines
    }

    fn format_todo_read_tool(
        &self,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if let Some(result) = result {
            // We have a result - show summary or details based on mode
            match result {
                ToolResult::Success { output } => {
                    if mode == ViewMode::Compact {
                        // Try to parse as JSON to show summary
                        if let Ok(todos) = serde_json::from_str::<Vec<serde_json::Value>>(output) {
                            let pending = todos
                                .iter()
                                .filter(|t| {
                                    t.get("status").and_then(|s| s.as_str()) == Some("pending")
                                })
                                .count();
                            let in_progress = todos
                                .iter()
                                .filter(|t| {
                                    t.get("status").and_then(|s| s.as_str()) == Some("in_progress")
                                })
                                .count();
                            lines.push(Line::from(Span::styled(
                                format!(
                                    "{} todos ({} pending, {} in progress)",
                                    todos.len(),
                                    pending,
                                    in_progress
                                ),
                                styles::DIM_TEXT,
                            )));
                        } else {
                            lines
                                .push(Line::from(Span::styled("Read todo list", styles::DIM_TEXT)));
                        }
                    } else {
                        // Detailed mode - show the todos
                        lines.push(Line::from(Span::styled("Todo List:", styles::TOOL_HEADER)));
                        if let Ok(todos) = serde_json::from_str::<Vec<serde_json::Value>>(output) {
                            for todo in &todos {
                                if let (Some(content), Some(status), Some(priority)) = (
                                    todo.get("content").and_then(|c| c.as_str()),
                                    todo.get("status").and_then(|s| s.as_str()),
                                    todo.get("priority").and_then(|p| p.as_str()),
                                ) {
                                    let status_icon = match status {
                                        "pending" => "⏳",
                                        "in_progress" => "🔄",
                                        "completed" => "✅",
                                        _ => "❓",
                                    };
                                    let priority_color = match priority {
                                        "high" => Color::Red,
                                        "medium" => Color::Yellow,
                                        "low" => Color::Green,
                                        _ => Color::White,
                                    };
                                    lines.push(Line::from(vec![
                                        Span::styled(format!("{} ", status_icon), Style::default()),
                                        Span::styled(
                                            format!("[{}] ", priority.to_uppercase()),
                                            Style::default().fg(priority_color),
                                        ),
                                        Span::styled(content.to_string(), Style::default()),
                                    ]));
                                }
                            }
                        } else {
                            // Fallback to raw output
                            for line in output.lines().take(20) {
                                for wrapped in textwrap::wrap(line, wrap_width) {
                                    lines.push(Line::from(Span::raw(wrapped.to_string())));
                                }
                            }
                        }
                    }
                }
                ToolResult::Error { error } => {
                    lines.push(Line::from(Span::styled(
                        format!("Error reading todos: {}", error),
                        styles::ERROR_TEXT,
                    )));
                }
            }
        } else {
            // No result yet - just show we're reading
            lines.push(Line::from(Span::styled("Read todo list", styles::DIM_TEXT)));
        }

        lines
    }

    fn format_todo_write_tool(
        &self,
        params: &TodoWriteParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let todo_count = params.todos.len();
        let completed_count = params
            .todos
            .iter()
            .filter(|t| t.status == tools::tools::todo::TodoStatus::Completed)
            .count();

        if mode == ViewMode::Compact || result.is_some() {
            // Compact mode or when we have a result - show single line summary
            lines.push(Line::from(Span::styled(
                format!(
                    "Update todos ({} items, {} completed)",
                    todo_count, completed_count
                ),
                styles::DIM_TEXT,
            )));
        } else {
            // Detailed mode without result - show full todo list
            lines.push(Line::from(Span::styled(
                format!("Updating {} todo items:", params.todos.len()),
                Style::default(),
            )));
            for todo in &params.todos {
                let status_icon = match todo.status {
                    tools::tools::todo::TodoStatus::Pending => "⏳",
                    tools::tools::todo::TodoStatus::InProgress => "🔄",
                    tools::tools::todo::TodoStatus::Completed => "✅",
                };
                let priority_color = match todo.priority {
                    tools::tools::todo::TodoPriority::High => Color::Red,
                    tools::tools::todo::TodoPriority::Medium => Color::Yellow,
                    tools::tools::todo::TodoPriority::Low => Color::Green,
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", status_icon), Style::default()),
                    Span::styled(
                        format!("[{}] ", format!("{:?}", todo.priority).to_uppercase()),
                        Style::default().fg(priority_color),
                    ),
                    Span::styled(todo.content.clone(), Style::default()),
                ]));
            }
        }

        lines
    }

    fn format_view_tool(
        &self,
        params: &ViewParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let file_name = std::path::Path::new(&params.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&params.file_path);

        if mode == ViewMode::Compact {
            // Compact mode - just show file name and result summary
            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let line_count = output.lines().count();
                        lines.push(Line::from(Span::styled(
                            format!("Read {} ({} lines)", file_name, line_count),
                            styles::DIM_TEXT,
                        )));
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Failed to read {}", file_name),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Read {}", file_name),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                format!("File: {}", params.file_path),
                Style::default(),
            )));

            // Add file info if available
            if let Some(offset) = params.offset {
                lines.push(Line::from(Span::styled(
                    format!("Starting from line: {}", offset),
                    styles::DIM_TEXT,
                )));
            }
            if let Some(limit) = params.limit {
                lines.push(Line::from(Span::styled(
                    format!("Max lines: {}", limit),
                    styles::DIM_TEXT,
                )));
            }

            // Show file content if we have a result
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_PREVIEW_LINES: usize = 20;
                    let content_lines: Vec<&str> = output.lines().collect();

                    for line in content_lines.iter().take(MAX_PREVIEW_LINES) {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::raw(wrapped.to_string())));
                        }
                    }

                    if content_lines.len() > MAX_PREVIEW_LINES {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "... ({} more lines)",
                                content_lines.len() - MAX_PREVIEW_LINES
                            ),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_ls_tool(
        &self,
        params: &LsParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let dir_name = std::path::Path::new(&params.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&params.path);

        if mode == ViewMode::Compact {
            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let file_count = output
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .count();
                        lines.push(Line::from(Span::styled(
                            format!("List {} ({} files)", dir_name, file_count),
                            styles::DIM_TEXT,
                        )));
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Failed to list {}", dir_name),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("List {}", dir_name),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                format!("Directory: {}", params.path),
                Style::default(),
            )));
            if let Some(ignore) = &params.ignore {
                lines.push(Line::from(Span::styled(
                    format!("Ignore patterns: {}", ignore.join(", ")),
                    styles::DIM_TEXT,
                )));
            }

            // Show files if we have results
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_FILES: usize = 20;
                    let files: Vec<&str> =
                        output.lines().filter(|l| !l.trim().is_empty()).collect();

                    for file in files.iter().take(MAX_FILES) {
                        lines.push(Line::from(Span::raw(file.to_string())));
                    }

                    if files.len() > MAX_FILES {
                        lines.push(Line::from(Span::styled(
                            format!("... ({} more files)", files.len() - MAX_FILES),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_glob_tool(
        &self,
        params: &GlobParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let path_display = params.path.as_deref().unwrap_or(".");

        if mode == ViewMode::Compact {
            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let file_count = output
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .count();
                        lines.push(Line::from(Span::styled(
                            format!(
                                "Glob '{}' in {} ({} matches)",
                                params.pattern, path_display, file_count
                            ),
                            styles::DIM_TEXT,
                        )));
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Glob '{}' failed", params.pattern),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Glob '{}' in {}", params.pattern, path_display),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                format!("Pattern: {}", params.pattern),
                Style::default(),
            )));
            if let Some(path) = &params.path {
                lines.push(Line::from(Span::styled(
                    format!("Path: {}", path),
                    Style::default(),
                )));
            }

            // Show matches if we have results
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_FILES: usize = 20;
                    let files: Vec<&str> =
                        output.lines().filter(|l| !l.trim().is_empty()).collect();

                    for file in files.iter().take(MAX_FILES) {
                        lines.push(Line::from(Span::raw(file.to_string())));
                    }

                    if files.len() > MAX_FILES {
                        lines.push(Line::from(Span::styled(
                            format!("... ({} more matches)", files.len() - MAX_FILES),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_replace_tool(
        &self,
        params: &ReplaceParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            if result.is_some() {
                lines.push(Line::from(Span::styled(
                    format!(
                        "Replace {} ({} lines)",
                        params.file_path,
                        params.content.lines().count()
                    ),
                    Style::default().fg(Color::Yellow),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Replace {}", params.file_path),
                    Style::default().fg(Color::Yellow),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                format!("Replacing {}", params.file_path),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));

            if result.is_none() {
                // Show preview of new content
                lines.push(Line::from(Span::styled(
                    format!("+++ {} (Full New Content)", params.file_path),
                    styles::TOOL_SUCCESS,
                )));
                const MAX_PREVIEW_LINES: usize = 15;
                for (idx, line) in params.content.lines().enumerate() {
                    if idx >= MAX_PREVIEW_LINES {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "... ({} more lines)",
                                params.content.lines().count() - MAX_PREVIEW_LINES
                            ),
                            styles::ITALIC_GRAY,
                        )));
                        break;
                    }
                    for wrapped_line in textwrap::wrap(line, wrap_width) {
                        lines.push(Line::from(Span::styled(
                            format!("+ {}", wrapped_line),
                            styles::TOOL_SUCCESS,
                        )));
                    }
                }
            }

            // Show error if result is an error
            if let Some(ToolResult::Error { error }) = result {
                lines.push(Line::from(Span::styled(
                    format!("Error: {}", error),
                    styles::ERROR_TEXT,
                )));
            }
        }

        lines
    }

    fn format_dispatch_agent_tool(
        &self,
        params: &DispatchAgentParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            let preview = if params.prompt.len() > 60 {
                format!("{}...", &params.prompt[..57])
            } else {
                params.prompt.clone()
            };

            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let line_count = output.lines().count();
                        lines.push(Line::from(Span::styled(
                            format!("Agent: {} ({} lines)", preview, line_count),
                            styles::DIM_TEXT,
                        )));
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Agent failed: {}", preview),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Agent: {}", preview),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled("Agent Task:", styles::TOOL_HEADER)));
            for line in params.prompt.lines() {
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        Style::default(),
                    )));
                }
            }

            // Show output if we have results
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_OUTPUT_LINES: usize = 30;
                    let output_lines: Vec<&str> = output.lines().collect();

                    for line in output_lines.iter().take(MAX_OUTPUT_LINES) {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::raw(wrapped.to_string())));
                        }
                    }

                    if output_lines.len() > MAX_OUTPUT_LINES {
                        lines.push(Line::from(Span::styled(
                            format!("... ({} more lines)", output_lines.len() - MAX_OUTPUT_LINES),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_fetch_tool(
        &self,
        params: &FetchParams,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Compact {
            if let Some(result) = result {
                match result {
                    ToolResult::Success { output } => {
                        let char_count = output.len();
                        let size_str = if char_count > 1000 {
                            format!("{:.1}KB", char_count as f64 / 1000.0)
                        } else {
                            format!("{} chars", char_count)
                        };
                        lines.push(Line::from(Span::styled(
                            format!("Fetch: {} ({})", params.url, size_str),
                            styles::DIM_TEXT,
                        )));
                    }
                    ToolResult::Error { .. } => {
                        lines.push(Line::from(Span::styled(
                            format!("Failed to fetch: {}", params.url),
                            styles::ERROR_TEXT,
                        )));
                    }
                }
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Fetch: {}", params.url),
                    styles::DIM_TEXT,
                )));
            }
        } else {
            // Detailed mode
            lines.push(Line::from(Span::styled(
                format!("URL: {}", params.url),
                Style::default(),
            )));
            lines.push(Line::from(Span::styled("Prompt:", styles::TOOL_HEADER)));
            for line in params.prompt.lines() {
                for wrapped_line in textwrap::wrap(line, wrap_width) {
                    lines.push(Line::from(Span::styled(
                        wrapped_line.to_string(),
                        Style::default(),
                    )));
                }
            }

            // Show output if we have results
            if let Some(ToolResult::Success { output }) = result {
                if !output.trim().is_empty() {
                    lines.push(Line::from(Span::styled(
                        "─".repeat(wrap_width.min(40)),
                        styles::DIM_TEXT,
                    )));

                    const MAX_OUTPUT_LINES: usize = 25;
                    let output_lines: Vec<&str> = output.lines().collect();

                    for line in output_lines.iter().take(MAX_OUTPUT_LINES) {
                        for wrapped in textwrap::wrap(line, wrap_width) {
                            lines.push(Line::from(Span::raw(wrapped.to_string())));
                        }
                    }

                    if output_lines.len() > MAX_OUTPUT_LINES {
                        lines.push(Line::from(Span::styled(
                            format!("... ({} more lines)", output_lines.len() - MAX_OUTPUT_LINES),
                            styles::ITALIC_GRAY,
                        )));
                    }
                }
            }
        }

        lines
    }

    fn format_default_tool(
        &self,
        call: &ToolCall,
        result: &Option<ToolResult>,
        wrap_width: usize,
        mode: ViewMode,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if mode == ViewMode::Detailed {
            // Show parameters
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
        } else {
            // Compact mode - just show tool name
            lines.push(Line::from(Span::styled(
                format!("{} tool", call.name),
                styles::DIM_TEXT,
            )));
        }

        // Show result if available
        if let Some(result) = result {
            if mode == ViewMode::Detailed {
                lines.push(Line::from(Span::styled(
                    "─".repeat(wrap_width.min(40)),
                    styles::DIM_TEXT,
                )));
            }

            match result {
                ToolResult::Success { output } => {
                    if output.trim().is_empty() {
                        if mode == ViewMode::Detailed {
                            lines
                                .push(Line::from(Span::styled("(No output)", styles::ITALIC_GRAY)));
                        }
                    } else {
                        const MAX_LINES: usize = 10;
                        let output_lines: Vec<&str> = output.lines().collect();

                        for line in output_lines.iter().take(MAX_LINES) {
                            for wrapped in textwrap::wrap(line, wrap_width) {
                                lines.push(Line::from(Span::raw(wrapped.to_string())));
                            }
                        }

                        if output_lines.len() > MAX_LINES {
                            lines.push(Line::from(Span::styled(
                                format!("... ({} more lines)", output_lines.len() - MAX_LINES),
                                styles::ITALIC_GRAY,
                            )));
                        }
                    }
                }
                ToolResult::Error { error } => {
                    lines.push(Line::from(Span::styled(
                        format!("Error: {}", error),
                        styles::ERROR_TEXT,
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
                    "Completed successfully",
                    styles::TOOL_SUCCESS,
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("Failed with exit code {}", exit_code),
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

        // Build title with status indicator first
        let status_indicator = if exit_code == 0 {
            Span::styled(" ✓ ", styles::TOOL_SUCCESS)
        } else {
            Span::styled(" ✗ ", styles::TOOL_ERROR)
        };

        let title = Line::from(vec![
            status_indicator,
            Span::styled("Command Execution", box_style),
        ]);

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

                // Tool content with integrated call/result
                let tool_lines = self.format_tool_with_result(
                    call,
                    result,
                    width.saturating_sub(4) as usize,
                    mode,
                );
                height += tool_lines.len() as u16;

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
