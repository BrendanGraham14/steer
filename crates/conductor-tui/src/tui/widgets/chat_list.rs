//! ChatList widget - simplified message list for the new data model

use crate::tui::model::{ChatItem, MessageRow, NoticeLevel};
use crate::tui::widgets::formatters;
use crate::tui::widgets::styles;
use conductor_core::app::conversation::{AssistantContent, Message, ToolResult};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, StatefulWidget, Widget, Wrap},
};
use textwrap;
use time::format_description::well_known::Rfc3339;

/// View mode for message rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    Compact,
    Detailed,
}

/// State for the ChatList widget
#[derive(Debug)]
pub struct ChatListState {
    /// Current scroll offset (row-based)
    pub offset: u16,
    /// View preferences
    pub view_mode: ViewMode,
    /// Cached visible range for efficient rendering
    visible_range: Option<VisibleRange>,
    /// Total content height (cached during render)
    total_content_height: u16,
    /// Viewport height (cached during render)
    last_viewport_height: u16,
    /// Track if user has manually scrolled away from bottom
    user_scrolled: bool,
}

#[derive(Debug, Clone)]
pub struct VisibleRange {
    pub first_index: usize,
    pub last_index: usize,
    pub first_y: u16,
    pub last_y: u16,
}

impl Default for ChatListState {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatListState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            view_mode: ViewMode::Compact,
            visible_range: None,
            total_content_height: 0,
            last_viewport_height: 0,
            user_scrolled: false,
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        // This will be calculated during render
        self.offset = u16::MAX;
        self.user_scrolled = false;
    }

    pub fn scroll_up(&mut self, amount: u16) {
        self.offset = self.offset.saturating_sub(amount);
        self.user_scrolled = true;
    }

    pub fn scroll_down(&mut self, amount: u16) {
        self.offset = self.offset.saturating_add(amount);
        self.user_scrolled = true;
    }

    pub fn scroll_to_top(&mut self) {
        self.offset = 0;
        self.user_scrolled = true;
    }

    pub fn is_at_bottom(&self) -> bool {
        // Check if we're at the bottom based on actual content height
        if self.total_content_height == 0 || self.last_viewport_height == 0 {
            return true;
        }

        let max_offset = self
            .total_content_height
            .saturating_sub(self.last_viewport_height);
        // We're at bottom if offset is at max or if user hasn't manually scrolled
        !self.user_scrolled || self.offset >= max_offset
    }

    /// Scroll to center a specific item in the viewport
    pub fn scroll_to_item(&mut self, index: usize) {
        // Mark that we need to scroll to a specific item
        // The actual scrolling will happen during render when we know item positions
        self.user_scrolled = true;
        // Use a special offset value to indicate we need to calculate scroll position for item
        self.offset = u16::MAX - 1 - (index as u16); // Encode the index in the offset
    }
}

/// The ChatList widget
pub struct ChatList<'a> {
    items: &'a [ChatItem],
    block: Option<Block<'a>>,
    hovered_message_id: Option<&'a str>,
}

impl<'a> ChatList<'a> {
    pub fn new(items: &'a [ChatItem]) -> Self {
        Self {
            items,
            block: None,
            hovered_message_id: None,
        }
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn hovered_message_id(mut self, id: Option<&'a str>) -> Self {
        self.hovered_message_id = id;
        self
    }

    fn render_gutter(&self, message: &MessageRow, is_hovered: bool) -> Line<'static> {
        let mut spans = vec![];

        // Role indicator
        let (symbol, color) = match &message.inner {
            Message::User { .. } => ("▶", Color::Blue),
            Message::Assistant { .. } => ("◀", Color::Green),
            Message::Tool { .. } => ("⚙", Color::Cyan),
        };

        let style = if is_hovered {
            // Hovered style - reversed or highlighted
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };

        spans.push(Span::styled(symbol, style));

        Line::from(spans)
    }

    fn render_meta_gutter(&self) -> Line<'static> {
        let style = Style::default().fg(Color::Gray);
        Line::from(vec![Span::styled("•", style)])
    }

    fn render_chat_item(
        &self,
        item: &ChatItem,
        width: u16,
        view_mode: ViewMode,
    ) -> (Vec<Line<'static>>, u16) {
        let max_width = (width.saturating_sub(4)) as usize; // 4 for gutter + minimal padding

        // Check if this message is hovered
        let is_hovered = if let ChatItem::Message(row) = item {
            self.hovered_message_id
                .map(|id| id == row.inner.id())
                .unwrap_or(false)
        } else {
            false
        };

        match item {
            ChatItem::Message(row) => {
                // Render the message content
                let mut lines = vec![];

                // Render the actual message based on type
                match &row.inner {
                    Message::User { content, .. } => {
                        // Handle empty content
                        if content.is_empty() {
                            lines.push(self.render_gutter(row, is_hovered));
                        } else {
                            // Render user content blocks
                            for (idx, block) in content.iter().enumerate() {
                                match block {
                                conductor_core::app::conversation::UserContent::Text { text } => {
                                    // For the first line, put it inline with the gutter
                                    let wrapped = textwrap::wrap(text, max_width);

                                    for (line_idx, wrapped_line) in wrapped.iter().enumerate() {
                                        if idx == 0 && line_idx == 0 {
                                            // First line goes with the gutter
                                            let mut first_line = vec![];
                                            first_line.extend(self.render_gutter(row, is_hovered).spans);
                                            first_line.push(Span::raw(" "));
                                            first_line.push(Span::raw(wrapped_line.to_string()));
                                            lines.push(Line::from(first_line));
                                        } else {
                                            // Continuation lines are indented
                                            lines.push(Line::from(vec![
                                                Span::raw("  "), // Indent to align with text after glyph
                                                Span::raw(wrapped_line.to_string()),
                                            ]));
                                        }
                                    }

                                    // Add empty line between blocks
                                    if idx + 1 < content.len() {
                                        lines.push(Line::from(""));
                                    }
                                }
                                conductor_core::app::conversation::UserContent::CommandExecution { command, stdout, stderr, exit_code } => {
                                    // Add gutter if this is the first block
                                    if idx == 0 {
                                        let mut cmd_line = vec![];
                                        cmd_line.extend(self.render_gutter(row, is_hovered).spans);
                                        cmd_line.push(Span::raw(" "));
                                        cmd_line.push(Span::styled("$ ", Style::default().fg(Color::Yellow)));
                                        cmd_line.push(Span::raw(command.clone()));
                                        lines.push(Line::from(cmd_line));
                                    } else {
                                        lines.push(Line::from(""));
                                        let mut cmd_line = vec![];
                                        cmd_line.extend(self.render_gutter(row, is_hovered).spans);
                                        cmd_line.push(Span::raw(" "));
                                        cmd_line.push(Span::styled("$ ", Style::default().fg(Color::Yellow)));
                                        cmd_line.push(Span::raw(command.clone()));
                                        lines.push(Line::from(cmd_line));
                                    }

                                    // Show exit code if non-zero
                                    if *exit_code != 0 {
                                        lines.push(Line::from(vec![
                                            Span::raw("  "),
                                            Span::styled(format!("Exit code: {}", exit_code), Style::default().fg(Color::Red)),
                                        ]));
                                    }

                                    // Show stdout if not empty
                                    if !stdout.is_empty() {
                                        let stdout_wrapped = textwrap::wrap(stdout, max_width.saturating_sub(2));
                                        for line in stdout_wrapped {
                                            lines.push(Line::from(vec![
                                                Span::raw("  "),
                                                Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                                            ]));
                                        }
                                    }

                                    // Show stderr if not empty
                                    if !stderr.is_empty() {
                                        lines.push(Line::from(vec![
                                            Span::raw("  "),
                                            Span::styled("Error:", Style::default().fg(Color::Red)),
                                        ]));
                                        let stderr_wrapped = textwrap::wrap(stderr, max_width.saturating_sub(2));
                                        for line in stderr_wrapped {
                                            lines.push(Line::from(vec![
                                                Span::raw("  "),
                                                Span::styled(line.to_string(), Style::default().fg(Color::Red)),
                                            ]));
                                        }
                                    }
                                }
                                conductor_core::app::conversation::UserContent::AppCommand { command, response } => {
                                    // Format command nicely
                                    let command_str = match command {
                                        conductor_core::app::conversation::AppCommandType::Model { target } => {
                                            if let Some(model) = target {
                                                format!("/model {}", model)
                                            } else {
                                                "/model".to_string()
                                            }
                                        }
                                        conductor_core::app::conversation::AppCommandType::Compact => "/compact".to_string(),
                                        conductor_core::app::conversation::AppCommandType::Clear => "/clear".to_string(),
                                        conductor_core::app::conversation::AppCommandType::Cancel => "/cancel".to_string(),
                                        conductor_core::app::conversation::AppCommandType::Help => "/help".to_string(),
                                        conductor_core::app::conversation::AppCommandType::Unknown { command } => command.clone(),
                                    };

                                    // Add gutter if this is the first block
                                    if idx == 0 {
                                        let mut cmd_line = vec![];
                                        cmd_line.extend(self.render_gutter(row, is_hovered).spans);
                                        cmd_line.push(Span::raw(" "));
                                        cmd_line.push(Span::styled(command_str, Style::default().fg(Color::Magenta)));
                                        lines.push(Line::from(cmd_line));
                                    } else {
                                        lines.push(Line::from(""));
                                        let mut cmd_line = vec![];
                                        cmd_line.extend(self.render_gutter(row, is_hovered).spans);
                                        cmd_line.push(Span::raw(" "));
                                        cmd_line.push(Span::styled(command_str, Style::default().fg(Color::Magenta)));
                                        lines.push(Line::from(cmd_line));
                                    }

                                    if let Some(resp) = response {
                                        match resp {
                                            conductor_core::app::conversation::CommandResponse::Text(text) => {
                                                let wrapped = textwrap::wrap(text, max_width.saturating_sub(2));
                                                for line in wrapped {
                                                    lines.push(Line::from(vec![
                                                        Span::raw("  "),
                                                        Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                                                    ]));
                                                }
                                            }
                                            conductor_core::app::conversation::CommandResponse::Compact(result) => {
                                                let text = match result {
                                                    conductor_core::app::conversation::CompactResult::Success(summary) => summary,
                                                    conductor_core::app::conversation::CompactResult::Cancelled => "Cancelled",
                                                    conductor_core::app::conversation::CompactResult::InsufficientMessages => "Not enough messages to compact.",
                                                };
                                                let wrapped = textwrap::wrap(text, max_width.saturating_sub(2));
                                                for line in wrapped {
                                                    lines.push(Line::from(vec![
                                                        Span::raw("  "),
                                                        Span::styled(line.to_string(), Style::default().fg(Color::Green)),
                                                    ]));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            }
                        }
                    }
                    Message::Assistant { content, .. } => {
                        let mut first_content_rendered = false;

                        // Render assistant content blocks
                        for (idx, block) in content.iter().enumerate() {
                            match block {
                                AssistantContent::Text { text } => {
                                    // Skip empty text blocks
                                    if text.trim().is_empty() {
                                        continue;
                                    }

                                    // Wrap the text properly
                                    let wrapped = textwrap::wrap(text, max_width.saturating_sub(3)); // Account for indent

                                    for (line_idx, wrapped_line) in wrapped.iter().enumerate() {
                                        if !first_content_rendered && line_idx == 0 {
                                            // First line goes with the indented gutter
                                            let mut first_line = vec![];
                                            first_line.push(Span::raw("  ")); // Indent the glyph
                                            first_line
                                                .extend(self.render_gutter(row, is_hovered).spans);
                                            first_line.push(Span::raw(" "));
                                            first_line.push(Span::raw(wrapped_line.to_string()));
                                            lines.push(Line::from(first_line));
                                            first_content_rendered = true;
                                        } else {
                                            // Continuation lines are indented
                                            lines.push(Line::from(vec![
                                                Span::raw("    "), // 4 spaces to align with text after indented glyph
                                                Span::raw(wrapped_line.to_string()),
                                            ]));
                                        }
                                    }

                                    // Add empty line between blocks
                                    if idx + 1 < content.len() {
                                        lines.push(Line::from(""));
                                    }
                                }
                                AssistantContent::ToolCall { tool_call } => {
                                    // Skip rendering tool calls here - they're shown as separate Tool messages
                                    continue;
                                }
                                AssistantContent::Thought { thought } => {
                                    // Wrap the thought text properly
                                    let thought_text = thought.display_text();
                                    let wrapped =
                                        textwrap::wrap(&thought_text, max_width.saturating_sub(3)); // 3 for indent

                                    for (line_idx, wrapped_line) in wrapped.iter().enumerate() {
                                        if !first_content_rendered && line_idx == 0 {
                                            // First line goes with the indented gutter
                                            let mut first_line = vec![];
                                            first_line.push(Span::raw("  ")); // Indent
                                            first_line
                                                .extend(self.render_gutter(row, is_hovered).spans);
                                            first_line.push(Span::raw(" "));
                                            first_line.push(Span::styled(
                                                wrapped_line.to_string(),
                                                Style::default()
                                                    .fg(Color::DarkGray)
                                                    .add_modifier(Modifier::ITALIC),
                                            ));
                                            lines.push(Line::from(first_line));
                                            first_content_rendered = true;
                                        } else {
                                            lines.push(Line::from(vec![
                                                Span::raw("    "),
                                                Span::styled(
                                                    wrapped_line.to_string(),
                                                    Style::default()
                                                        .fg(Color::DarkGray)
                                                        .add_modifier(Modifier::ITALIC),
                                                ),
                                            ]));
                                        }
                                    }
                                }
                            }
                        }

                        // If no content was rendered, just show the glyph
                        if !first_content_rendered {
                            let mut glyph_line = vec![];
                            glyph_line.push(Span::raw("  ")); // Indent
                            glyph_line.extend(self.render_gutter(row, is_hovered).spans);
                            lines.push(Line::from(glyph_line));
                        }
                    }
                    Message::Tool {
                        tool_use_id,
                        result,
                        ..
                    } => {
                        // Get the tool call from the registry
                        if let Some(item) = self.items.iter().find(|item| {
                            if let ChatItem::Message(msg_row) = item {
                                if let Message::Assistant { content, .. } = &msg_row.inner {
                                    content.iter().any(|block| {
                                        if let AssistantContent::ToolCall { tool_call } = block {
                                            tool_call.id == *tool_use_id
                                        } else {
                                            false
                                        }
                                    })
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        }) {
                            if let ChatItem::Message(msg_row) = item {
                                if let Message::Assistant { content, .. } = &msg_row.inner {
                                    // Find the specific tool call
                                    if let Some(tool_call) = content.iter().find_map(|block| {
                                        if let AssistantContent::ToolCall { tool_call } = block {
                                            if tool_call.id == *tool_use_id {
                                                Some(tool_call)
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    }) {
                                        // Use the formatter to render the tool
                                        let formatter = formatters::get_formatter(&tool_call.name);
                                        let wrap_width = (width.saturating_sub(7)) as usize; // Account for indent and padding

                                        let formatted_lines = match view_mode {
                                            ViewMode::Compact => formatter.compact(
                                                &tool_call.parameters,
                                                &Some(result.clone()),
                                                wrap_width,
                                            ),
                                            ViewMode::Detailed => formatter.detailed(
                                                &tool_call.parameters,
                                                &Some(result.clone()),
                                                wrap_width,
                                            ),
                                        };

                                        // First line with indented gutter and tool name
                                        let mut first_line = vec![];
                                        first_line.push(Span::raw("  ")); // Indent
                                        first_line
                                            .extend(self.render_gutter(row, is_hovered).spans);
                                        first_line.push(Span::raw(" "));
                                        first_line.push(Span::styled(
                                            tool_call.name.clone(),
                                            Style::default().fg(Color::Cyan),
                                        ));

                                        // For compact mode, try to fit first output on same line
                                        if view_mode == ViewMode::Compact
                                            && !formatted_lines.is_empty()
                                        {
                                            let first_output = &formatted_lines[0];
                                            if first_output.width()
                                                < (wrap_width - tool_call.name.len() - 5)
                                            {
                                                // Add status indicator or bullet
                                                match result {
                                                    ToolResult::Success { .. } => {
                                                        first_line.push(Span::styled(
                                                            " ✓ ",
                                                            styles::TOOL_SUCCESS,
                                                        ));
                                                    }
                                                    ToolResult::Error { .. } => {
                                                        first_line.push(Span::styled(
                                                            " ✗ ",
                                                            styles::ERROR_TEXT,
                                                        ));
                                                    }
                                                }
                                                first_line.extend(first_output.spans.clone());
                                                lines.push(Line::from(first_line));

                                                // Add remaining lines
                                                for line in formatted_lines.iter().skip(1) {
                                                    let mut indented_spans =
                                                        vec![Span::raw("    ")]; // Align with content
                                                    indented_spans.extend(line.spans.clone());
                                                    lines.push(Line::from(indented_spans));
                                                }
                                            } else {
                                                // Add status indicator on first line if we have result
                                                if view_mode == ViewMode::Compact {
                                                    match result {
                                                        ToolResult::Success { .. } => {
                                                            first_line.push(Span::styled(
                                                                " ✓",
                                                                styles::TOOL_SUCCESS,
                                                            ));
                                                        }
                                                        ToolResult::Error { .. } => {
                                                            first_line.push(Span::styled(
                                                                " ✗",
                                                                styles::ERROR_TEXT,
                                                            ));
                                                        }
                                                    }
                                                }
                                                lines.push(Line::from(first_line));
                                                // Add all lines with indent
                                                for line in formatted_lines {
                                                    let mut indented_spans =
                                                        vec![Span::raw("    ")]; // Align with content
                                                    indented_spans.extend(line.spans.clone());
                                                    lines.push(Line::from(indented_spans));
                                                }
                                            }
                                        } else {
                                            lines.push(Line::from(first_line));
                                            // Add the formatted output with proper indentation
                                            for line in formatted_lines {
                                                let mut indented_spans = vec![Span::raw("    ")]; // Align with content
                                                indented_spans.extend(line.spans.clone());
                                                lines.push(Line::from(indented_spans));
                                            }
                                        }

                                        let height = lines.len() as u16;
                                        return (lines, height);
                                    }
                                }
                            }
                        }

                        // Fallback if we can't find the tool call (shouldn't happen)
                        let mut fallback_line = vec![];
                        fallback_line.push(Span::raw("  ")); // Indent
                        fallback_line.extend(self.render_gutter(row, is_hovered).spans);
                        fallback_line.push(Span::raw(" "));
                        fallback_line.push(Span::styled(
                            "Tool Result",
                            Style::default().fg(Color::Cyan),
                        ));
                        lines.push(Line::from(fallback_line));

                        let (content, color) = match result {
                            ToolResult::Success { output } => (output, Color::Cyan),
                            ToolResult::Error { error } => (error, Color::Red),
                        };

                        let preview = if view_mode == ViewMode::Compact && content.len() > 100 {
                            format!("{}...", &content[..100])
                        } else {
                            content.clone()
                        };

                        for line in preview.lines() {
                            lines.push(Line::from(vec![
                                Span::raw("    "),
                                Span::styled(line.to_string(), Style::default().fg(color)),
                            ]));
                        }
                    }
                }

                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::SlashInput { raw, .. } => {
                let mut lines = vec![];

                // Render gutter and slash command on same line
                let mut first_line = vec![];
                first_line.extend(self.render_meta_gutter().spans);
                first_line.push(Span::raw(" "));
                first_line.push(Span::styled(
                    raw.clone(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::from(first_line));

                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::CmdResponse { cmd, resp, .. } => {
                let mut lines = vec![];

                // Format command nicely
                let command_str = match cmd {
                    conductor_core::app::conversation::AppCommandType::Model { target } => {
                        if let Some(model) = target {
                            format!("/model {}", model)
                        } else {
                            "/model".to_string()
                        }
                    }
                    conductor_core::app::conversation::AppCommandType::Compact => {
                        "/compact".to_string()
                    }
                    conductor_core::app::conversation::AppCommandType::Clear => {
                        "/clear".to_string()
                    }
                    conductor_core::app::conversation::AppCommandType::Cancel => {
                        "/cancel".to_string()
                    }
                    conductor_core::app::conversation::AppCommandType::Help => "/help".to_string(),
                    conductor_core::app::conversation::AppCommandType::Unknown { command } => {
                        command.clone()
                    }
                };

                // Get the full response text
                let response_text = match resp {
                    conductor_core::app::conversation::CommandResponse::Text(text) => {
                        text.clone()
                    }
                    conductor_core::app::conversation::CommandResponse::Compact(result) => {
                        match result {
                            conductor_core::app::conversation::CompactResult::Success(summary) => {
                                format!("Compact completed.\n\n{}", summary)
                            }
                            conductor_core::app::conversation::CompactResult::Cancelled => {
                                "Compact cancelled.".to_string()
                            }
                            conductor_core::app::conversation::CompactResult::InsufficientMessages => {
                                "Not enough messages to compact.".to_string()
                            }
                        }
                    }
                };

                // Split response into lines
                let response_lines: Vec<&str> = response_text.lines().collect();

                if response_lines.is_empty()
                    || (response_lines.len() == 1 && response_lines[0].len() <= 50)
                {
                    // Single short line - render inline
                    let mut first_line = vec![];
                    first_line.extend(self.render_meta_gutter().spans);
                    first_line.push(Span::raw(" "));
                    first_line.push(Span::styled(
                        command_str.to_string(),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ));
                    first_line.push(Span::raw(": "));
                    first_line.push(Span::styled(
                        response_text,
                        Style::default().fg(Color::White),
                    ));
                    lines.push(Line::from(first_line));
                } else {
                    // Multi-line or long response - render command and response separately
                    let mut first_line = vec![];
                    first_line.extend(self.render_meta_gutter().spans);
                    first_line.push(Span::raw(" "));
                    first_line.push(Span::styled(
                        command_str.to_string(),
                        Style::default()
                            .fg(Color::Magenta)
                            .add_modifier(Modifier::BOLD),
                    ));
                    first_line.push(Span::raw(":"));
                    lines.push(Line::from(first_line));

                    // Add response lines with proper indentation
                    for line in response_lines {
                        let wrapped = textwrap::wrap(line, max_width.saturating_sub(4));
                        if wrapped.is_empty() {
                            // Empty line
                            lines.push(Line::from(""));
                        } else {
                            for wrapped_line in wrapped {
                                lines.push(Line::from(vec![
                                    Span::raw("    "),
                                    Span::styled(
                                        wrapped_line.to_string(),
                                        Style::default().fg(Color::White),
                                    ),
                                ]));
                            }
                        }
                    }
                }

                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::SystemNotice {
                level, text, ts, ..
            } => {
                let mut lines = vec![];

                let (prefix, color) = match level {
                    NoticeLevel::Info => ("info: ", Color::Blue),
                    NoticeLevel::Warn => ("warn: ", Color::Yellow),
                    NoticeLevel::Error => ("error: ", Color::Red),
                };

                // Format timestamp
                let time_str = ts
                    .format(&Rfc3339)
                    .unwrap_or_else(|_| "unknown".to_string());

                // Render gutter and notice on same line
                let mut first_line = vec![];
                first_line.extend(self.render_meta_gutter().spans);
                first_line.push(Span::raw(" "));
                first_line.push(Span::styled(prefix, Style::default().fg(color)));
                first_line.push(Span::raw(text.clone()));
                first_line.push(Span::styled(
                    format!(" ({})", time_str),
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                ));
                lines.push(Line::from(first_line));

                let height = lines.len() as u16;
                (lines, height)
            }
        }
    }
}

impl StatefulWidget for ChatList<'_> {
    type State = ChatListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Render the block if present
        let list_area = if let Some(ref block) = self.block {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        if list_area.width < 3 || list_area.height == 0 {
            return;
        }

        // Calculate total height and item positions
        let mut item_positions = Vec::new();
        let mut total_height = 0u16;

        for (idx, item) in self.items.iter().enumerate() {
            let (_, height) = self.render_chat_item(item, list_area.width, state.view_mode);

            item_positions.push((idx, total_height, height));
            total_height += height;

            // Add spacing between messages (but not after the last one)
            if idx + 1 < self.items.len() {
                // Check if this is a message (not a meta item)
                if matches!(item, ChatItem::Message(_)) {
                    total_height += 1; // Add 1 line of spacing between messages
                }
            }
        }

        // Cache the total height and viewport height
        state.total_content_height = total_height;
        state.last_viewport_height = list_area.height;

        // Adjust scroll offset if needed
        if state.offset == u16::MAX || total_height <= list_area.height {
            // Scroll to bottom or fit in view
            state.offset = total_height.saturating_sub(list_area.height);
            state.user_scrolled = false;
        } else if state.offset > u16::MAX - 100 {
            // Special case: scroll to specific item
            // Decode the index from the offset
            let target_idx = (u16::MAX - 1 - state.offset) as usize;

            if let Some(&(_, y, height)) = item_positions.get(target_idx) {
                // Try to center the item
                let half_viewport = list_area.height / 2;
                let item_center = y + height / 2;

                if item_center > half_viewport {
                    state.offset = item_center - half_viewport;
                } else {
                    state.offset = 0;
                }

                // Ensure we don't scroll past the bottom
                state.offset = state
                    .offset
                    .min(total_height.saturating_sub(list_area.height));
            }
        } else {
            // Ensure we don't scroll past the bottom
            state.offset = state
                .offset
                .min(total_height.saturating_sub(list_area.height));
        }

        // Find visible items
        let visible_start = state.offset;
        let visible_end = (state.offset + list_area.height).min(total_height);

        let mut first_visible = None;
        let mut last_visible = None;

        for &(idx, y, height) in &item_positions {
            let item_end = y + height;

            // Check if item is at least partially visible
            if item_end > visible_start && y < visible_end {
                if first_visible.is_none() {
                    first_visible = Some((idx, y));
                }
                last_visible = Some((idx, y));
            }
        }

        // Update visible range in state
        if let (Some((first_idx, first_y)), Some((last_idx, last_y))) =
            (first_visible, last_visible)
        {
            state.visible_range = Some(VisibleRange {
                first_index: first_idx,
                last_index: last_idx,
                first_y,
                last_y,
            });
        }

        // Render visible items
        if let Some((first_idx, _)) = first_visible {
            let mut current_y = 0;

            for &(idx, item_y, _) in &item_positions[first_idx..] {
                if item_y >= visible_end {
                    break;
                }

                let item = &self.items[idx];
                let (lines, _) = self.render_chat_item(item, list_area.width, state.view_mode);

                // Calculate where to start rendering this item
                if item_y < visible_start {
                    // Item starts above visible area, skip some lines
                    let skip_lines = (visible_start - item_y) as usize;
                    if skip_lines < lines.len() {
                        for line in lines.iter().skip(skip_lines) {
                            if current_y < list_area.height {
                                let y = list_area.y + current_y;
                                let x = list_area.x;

                                // Render the line
                                let paragraph =
                                    Paragraph::new(line.clone()).wrap(Wrap { trim: false });

                                let line_area = Rect {
                                    x,
                                    y,
                                    width: list_area.width,
                                    height: 1,
                                };

                                paragraph.render(line_area, buf);
                                current_y += 1;
                            }
                        }
                    }
                } else {
                    // Normal rendering
                    for line in lines {
                        if current_y >= list_area.height {
                            break;
                        }

                        let y = list_area.y + current_y;
                        let x = list_area.x;

                        // Render the line
                        let paragraph = Paragraph::new(line).wrap(Wrap { trim: false });

                        let line_area = Rect {
                            x,
                            y,
                            width: list_area.width,
                            height: 1,
                        };

                        paragraph.render(line_area, buf);
                        current_y += 1;
                    }
                };

                // Add spacing between messages (but not after the last item)
                if idx + 1 < self.items.len()
                    && matches!(item, ChatItem::Message(_))
                    && current_y < list_area.height
                {
                    // Render an empty line for spacing
                    current_y += 1;
                }
            }
        }
    }
}
