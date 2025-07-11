//! ChatList widget - simplified message list for the new data model

use crate::tui::model::{ChatItem, MessageRow, NoticeLevel};
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::formatters;
use crate::tui::widgets::formatters::helpers::style_wrap;
use conductor_core::app::conversation::{AssistantContent, Message, ToolResult, UserContent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, StatefulWidget, Widget, Wrap},
};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use textwrap;
use time::format_description::well_known::Rfc3339;

/// Cache for rendered message lines
#[derive(Debug, Clone)]
pub struct RenderCache {
    pub lines: Vec<Line<'static>>,
    pub height: u16,
}

/// Key for the render cache (message ID, width, view mode)
type CacheKey = (String, u16, ViewMode);

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
    /// Cache for rendered message lines
    line_cache: HashMap<CacheKey, RenderCache>,
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
            line_cache: HashMap::new(),
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
    theme: &'a Theme,
    spinner_state: usize,
}

impl<'a> ChatList<'a> {
    pub fn new(items: &'a [ChatItem], theme: &'a Theme) -> Self {
        Self {
            items,
            block: None,
            hovered_message_id: None,
            theme,
            spinner_state: 0,
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

    /// Helper to wrap text and render lines with gutter
    fn wrap_with_gutter(&self, text: &str, wrap_width: usize, style: Style) -> Vec<Line<'static>> {
        let mut lines = vec![];
        let wrapped = textwrap::wrap(text, wrap_width);

        for (i, wrapped_line) in wrapped.into_iter().enumerate() {
            let mut line_spans = vec![];

            if i == 0 {
                // First line gets the gutter
                line_spans.extend(self.render_meta_gutter().spans);
                line_spans.push(Span::raw(" "));
            } else {
                // Continuation lines get spacing to align
                line_spans.push(Span::raw("  ")); // Two spaces to match gutter + space
            }

            line_spans.push(Span::styled(wrapped_line.to_string(), style));
            lines.push(Line::from(line_spans));
        }

        lines
    }

    pub fn spinner_state(mut self, state: usize) -> Self {
        self.spinner_state = state;
        self
    }

    fn render_gutter(&self, message: &MessageRow, is_hovered: bool) -> Line<'static> {
        let mut spans = vec![];

        // Role indicator
        let (symbol, component) = match &message.inner {
            Message::User { .. } => ("▶", Component::UserMessageRole),
            Message::Assistant { .. } => ("◀", Component::AssistantMessageRole),
            Message::Tool { .. } => ("⚙", Component::ToolCall),
        };

        let mut style = self.theme.style(component);

        if is_hovered {
            // Hovered style - add bold
            style = style.add_modifier(Modifier::BOLD);
        }

        spans.push(Span::styled(symbol, style));

        Line::from(spans)
    }

    fn render_meta_gutter(&self) -> Line<'static> {
        let style = self.theme.style(Component::DimText);
        Line::from(vec![Span::styled("•", style)])
    }

    fn get_item_cache_key(item: &ChatItem, width: u16, view_mode: ViewMode) -> CacheKey {
        match item {
            ChatItem::Message(row) => (row.id().to_string(), width, view_mode),
            _ => {
                // For non-message items, create a hash of the item content
                let mut hasher = DefaultHasher::new();
                format!("{item:?}").hash(&mut hasher);
                let hash = hasher.finish();
                (format!("item_{hash}"), width, view_mode)
            }
        }
    }

    fn render_chat_items(
        &self,
        item: &ChatItem,
        width: u16,
        view_mode: ViewMode,
        cache: Option<&mut HashMap<CacheKey, RenderCache>>,
    ) -> (Vec<Line<'static>>, u16) {
        // Skip caching for dynamic items that animate each frame
        if matches!(
            item,
            ChatItem::InFlightOperation { .. } | ChatItem::PendingToolCall { .. }
        ) {
            return self.render_item_uncached(item, width, view_mode);
        }
        // Try cache first if available
        if let Some(cache_map) = cache {
            let key = Self::get_item_cache_key(item, width, view_mode);
            let cache_entry = cache_map.entry(key).or_insert_with(|| {
                let (lines, height) = self.render_item_uncached(item, width, view_mode);
                RenderCache { lines, height }
            });
            return (cache_entry.lines.clone(), cache_entry.height);
        }

        // No cache, render directly
        self.render_item_uncached(item, width, view_mode)
    }

    fn render_item_uncached(
        &self,
        item: &ChatItem,
        width: u16,
        view_mode: ViewMode,
    ) -> (Vec<Line<'static>>, u16) {
        let max_width = (width.saturating_sub(4)) as usize; // 4 for gutter + minimal padding

        match item {
            ChatItem::Message(row) => {
                let cache = self.build_cache_for_message(row, width, view_mode);
                (cache.lines, cache.height)
            }

            ChatItem::PendingToolCall { tool_call, .. } => {
                // Get spinner frame based on current state
                let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let frame_idx = self.spinner_state % spinner_frames.len();
                let spinner = spinner_frames[frame_idx];

                // Render the pending tool call with spinner
                let first_line = vec![
                    Span::raw("  "), // Indent
                    Span::styled(
                        spinner,
                        self.theme
                            .style(Component::ToolCall)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        tool_call.name.clone(),
                        self.theme.style(Component::ToolCallHeader),
                    ),
                    Span::styled(" ⋯ ", self.theme.style(Component::DimText)),
                ];

                let formatter = formatters::get_formatter(&tool_call.name);
                let params = tool_call.parameters.clone();
                let result = None;
                let formatted_params_lines =
                    formatter.compact(&params, &result, max_width, self.theme);

                let mut lines = Vec::new();
                if let Some((first, rest)) = formatted_params_lines.split_first() {
                    // Compose the first line by joining the header and the first formatted param line
                    let mut first_line = first_line.clone();
                    // If the first formatted param line has any spans, append them
                    first_line.extend(first.spans.clone());
                    lines.push(Line::from(first_line));
                    // Add the rest of the formatted param lines as-is
                    for l in rest {
                        lines.push(l.clone());
                    }
                } else {
                    // No formatted params, just use the header line
                    lines.push(Line::from(first_line));
                }

                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::SlashInput { raw, .. } => {
                let lines = self.wrap_with_gutter(
                    raw,
                    max_width.saturating_sub(2),
                    self.theme.style(Component::CommandPrompt),
                );
                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::CmdResponse { cmd, resp, .. } => {
                let mut lines = vec![];

                // Format command nicely
                let command_str = match cmd {
                    conductor_core::app::conversation::AppCommandType::Model { target } => {
                        if let Some(model) = target {
                            format!("/model {model}")
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
                    conductor_core::app::conversation::AppCommandType::Auth => "/auth".to_string(),
                };

                // Get the full response text
                let response_text = match resp {
                    conductor_core::app::conversation::CommandResponse::Text(text) => {
                        text.clone()
                    }
                    conductor_core::app::conversation::CommandResponse::Compact(result) => {
                        match result {
                            conductor_core::app::conversation::CompactResult::Success(summary) => {
                                summary.clone()
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
                        self.theme.style(Component::CommandPrompt),
                    ));
                    first_line.push(Span::raw(": "));
                    first_line.push(Span::styled(
                        response_text,
                        self.theme.style(Component::CommandText),
                    ));
                    lines.push(Line::from(first_line));
                } else {
                    // Multi-line or long response - render command and response separately
                    let mut first_line = vec![];
                    first_line.extend(self.render_meta_gutter().spans);
                    first_line.push(Span::raw(" "));
                    first_line.push(Span::styled(
                        command_str.to_string(),
                        self.theme.style(Component::CommandPrompt),
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
                                        self.theme.style(Component::CommandText),
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
                let (prefix, component) = match level {
                    NoticeLevel::Info => ("info: ", Component::NoticeInfo),
                    NoticeLevel::Warn => ("warn: ", Component::NoticeWarn),
                    NoticeLevel::Error => ("error: ", Component::NoticeError),
                };

                // Format timestamp
                let time_str = ts
                    .format(&Rfc3339)
                    .unwrap_or_else(|_| "unknown".to_string());

                // Build the full notice text
                let full_text = format!("{prefix}{text} ({time_str})");

                // Calculate wrap width (account for gutter + space)
                let wrap_width = max_width.saturating_sub(2);

                // For system notices, we need special handling for prefix coloring
                let mut lines = vec![];
                let wrapped = textwrap::wrap(&full_text, wrap_width);

                for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                    let mut line_spans = vec![];

                    if i == 0 {
                        // First line - add gutter
                        line_spans.extend(self.render_meta_gutter().spans);
                        line_spans.push(Span::raw(" "));

                        // Add colored prefix
                        if let Some(stripped) = wrapped_line.strip_prefix(prefix) {
                            line_spans.push(Span::styled(prefix, self.theme.style(component)));
                            line_spans.push(Span::raw(stripped.to_string()));
                        } else {
                            line_spans.push(Span::raw(wrapped_line.to_string()));
                        }
                    } else {
                        // Continuation lines - just spacing, no gutter
                        line_spans.push(Span::raw("  ")); // Two spaces to align with gutter + space
                        line_spans.push(Span::raw(wrapped_line.to_string()));
                    }

                    lines.push(Line::from(line_spans));
                }

                let height = lines.len() as u16;
                (lines, height)
            }

            ChatItem::InFlightOperation { label, .. } => {
                let mut lines = vec![];

                // Get spinner frame based on current state
                let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let frame_idx = self.spinner_state % spinner_frames.len();
                let spinner = spinner_frames[frame_idx];

                // Render gutter and spinner with label
                let mut first_line = vec![];
                first_line.extend(self.render_meta_gutter().spans);
                first_line.push(Span::raw(" "));
                first_line.push(Span::styled(
                    spinner,
                    self.theme
                        .style(Component::TodoInProgress)
                        .add_modifier(Modifier::BOLD),
                ));
                first_line.push(Span::raw(" "));
                first_line.push(Span::styled(
                    label.clone(),
                    self.theme.style(Component::TodoInProgress),
                ));
                lines.push(Line::from(first_line));

                let height = lines.len() as u16;
                (lines, height)
            }
        }
    }

    /// Build cache for a single message row
    fn build_cache_for_message(
        &self,
        row: &MessageRow,
        width: u16,
        view_mode: ViewMode,
    ) -> RenderCache {
        let mut all_lines = Vec::new();
        let is_hovered = self.hovered_message_id == Some(row.id());
        let max_width = width.saturating_sub(4) as usize; // 4 for gutter + minimal padding

        match &row.inner {
            Message::User { content, .. } => {
                if content.is_empty() {
                    // Empty message - just show gutter
                    all_lines.push(self.render_gutter(row, is_hovered));
                } else {
                    // Process each content block
                    for (idx, block) in content.iter().enumerate() {
                        match block {
                            UserContent::Text { text } => {
                                // Parse markdown
                                let markdown_styles =
                                    super::markdown::MarkdownStyles::from_theme(self.theme);
                                let markdown_text = super::markdown::from_str_with_width(
                                    text,
                                    &markdown_styles,
                                    self.theme,
                                    Some(max_width as u16),
                                );

                                // Process each line from markdown
                                for (line_idx, marked_line) in
                                    markdown_text.lines.into_iter().enumerate()
                                {
                                    // Check if this line should be wrapped
                                    let lines_to_render = if marked_line.no_wrap {
                                        // Don't wrap code block lines
                                        vec![marked_line.line]
                                    } else {
                                        // Wrap the line while preserving styles
                                        style_wrap(marked_line.line, max_width as u16)
                                    };

                                    for (wrap_idx, line) in lines_to_render.into_iter().enumerate()
                                    {
                                        if idx == 0 && line_idx == 0 && wrap_idx == 0 {
                                            // First line gets the gutter
                                            let mut first_line =
                                                self.render_gutter(row, is_hovered).spans;
                                            first_line.push(Span::raw(" "));
                                            first_line.extend(line.spans);
                                            all_lines.push(Line::from(first_line));
                                        } else {
                                            // Continuation lines are indented
                                            let mut indented = vec![Span::raw("  ")];
                                            indented.extend(line.spans);
                                            all_lines.push(Line::from(indented));
                                        }
                                    }
                                }

                                // Add spacing between blocks
                                if idx + 1 < content.len() {
                                    all_lines.push(Line::from(""));
                                }
                            }
                            UserContent::CommandExecution {
                                command,
                                stdout,
                                stderr,
                                exit_code,
                            } => {
                                // Add gutter if this is the first block
                                if idx == 0 {
                                    let mut cmd_line = self.render_gutter(row, is_hovered).spans;
                                    cmd_line.push(Span::raw(" "));
                                    cmd_line.push(Span::styled(
                                        "$ ",
                                        self.theme.style(Component::CommandPrompt),
                                    ));
                                    cmd_line.push(Span::raw(command.clone()));
                                    all_lines.push(Line::from(cmd_line));
                                } else {
                                    all_lines.push(Line::from(""));
                                    let mut cmd_line = vec![Span::raw("  ")]; // Indent
                                    cmd_line.push(Span::styled(
                                        "$ ",
                                        self.theme.style(Component::CommandPrompt),
                                    ));
                                    cmd_line.push(Span::raw(command.clone()));
                                    all_lines.push(Line::from(cmd_line));
                                }

                                // Show exit code if non-zero
                                if *exit_code != 0 {
                                    all_lines.push(Line::from(vec![
                                        Span::raw("  "),
                                        Span::styled(
                                            format!("Exit code: {exit_code}"),
                                            self.theme.style(Component::ErrorText),
                                        ),
                                    ]));
                                }

                                // Show stdout if not empty
                                if !stdout.is_empty() {
                                    let stdout_wrapped =
                                        textwrap::wrap(stdout, max_width.saturating_sub(2));
                                    for line in stdout_wrapped {
                                        all_lines.push(Line::from(vec![
                                            Span::raw("  "),
                                            Span::styled(
                                                line.to_string(),
                                                self.theme.style(Component::DimText),
                                            ),
                                        ]));
                                    }
                                }

                                // Show stderr if not empty
                                if !stderr.is_empty() {
                                    all_lines.push(Line::from(vec![
                                        Span::raw("  "),
                                        Span::styled(
                                            "Error:",
                                            self.theme.style(Component::ErrorBold),
                                        ),
                                    ]));
                                    let stderr_wrapped =
                                        textwrap::wrap(stderr, max_width.saturating_sub(2));
                                    for line in stderr_wrapped {
                                        all_lines.push(Line::from(vec![
                                            Span::raw("  "),
                                            Span::styled(
                                                line.to_string(),
                                                self.theme.style(Component::ErrorText),
                                            ),
                                        ]));
                                    }
                                }

                                // Add spacing between blocks
                                if idx + 1 < content.len() {
                                    all_lines.push(Line::from(""));
                                }
                            }
                            UserContent::AppCommand { command, response } => {
                                // Format command nicely
                                let command_str = match command {
                                    conductor_core::app::conversation::AppCommandType::Model {
                                        target,
                                    } => {
                                        if let Some(model) = target {
                                            format!("/model {model}")
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
                                    conductor_core::app::conversation::AppCommandType::Help => {
                                        "/help".to_string()
                                    }
                                    conductor_core::app::conversation::AppCommandType::Auth => {
                                        "/auth".to_string()
                                    }
                                };

                                // Add gutter if this is the first block
                                if idx == 0 {
                                    let mut cmd_line = self.render_gutter(row, is_hovered).spans;
                                    cmd_line.push(Span::raw(" "));
                                    cmd_line.push(Span::styled(
                                        command_str,
                                        self.theme.style(Component::CommandPrompt),
                                    ));
                                    all_lines.push(Line::from(cmd_line));
                                } else {
                                    all_lines.push(Line::from(""));
                                    let mut cmd_line = vec![Span::raw("  ")]; // Indent
                                    cmd_line.push(Span::styled(
                                        command_str,
                                        self.theme.style(Component::CommandPrompt),
                                    ));
                                    all_lines.push(Line::from(cmd_line));
                                }

                                if let Some(resp) = response {
                                    match resp {
                                        conductor_core::app::conversation::CommandResponse::Text(text) => {
                                            let wrapped = textwrap::wrap(text, max_width.saturating_sub(2));
                                            for line in wrapped {
                                                all_lines.push(Line::from(vec![
                                                    Span::raw("  "),
                                                    Span::styled(line.to_string(), self.theme.style(Component::DimText)),
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
                                                all_lines.push(Line::from(vec![
                                                    Span::raw("  "),
                                                    Span::styled(line.to_string(), self.theme.style(Component::ToolSuccess)),
                                                ]));
                                            }
                                        }
                                    }
                                }

                                // Add spacing between blocks
                                if idx + 1 < content.len() {
                                    all_lines.push(Line::from(""));
                                }
                            }
                        }
                    }
                }
            }
            Message::Assistant { content, .. } => {
                let mut first_content_rendered = false;

                for (idx, block) in content.iter().enumerate() {
                    match block {
                        AssistantContent::Text { text } => {
                            if text.trim().is_empty() {
                                continue;
                            }

                            // Parse markdown
                            let markdown_styles =
                                super::markdown::MarkdownStyles::from_theme(self.theme);
                            let markdown_text = super::markdown::from_str_with_width(
                                text,
                                &markdown_styles,
                                self.theme,
                                Some(max_width.saturating_sub(3) as u16),
                            );

                            // Process each line
                            for marked_line in markdown_text.lines {
                                // Check if this line should be wrapped
                                let lines_to_render = if marked_line.no_wrap {
                                    // Don't wrap code block lines
                                    vec![marked_line.line]
                                } else {
                                    // Wrap with reduced width for assistant indent (3 for indent)
                                    style_wrap(marked_line.line, max_width.saturating_sub(3) as u16)
                                };

                                for line in lines_to_render {
                                    if !first_content_rendered {
                                        // First line gets indented gutter
                                        let mut first_line = vec![Span::raw("  ")]; // Indent
                                        first_line
                                            .extend(self.render_gutter(row, is_hovered).spans);
                                        first_line.push(Span::raw(" "));
                                        first_line.extend(line.spans);
                                        all_lines.push(Line::from(first_line));
                                        first_content_rendered = true;
                                    } else {
                                        // Continuation lines with more indent
                                        let mut indented = vec![Span::raw("    ")]; // 4 spaces
                                        indented.extend(line.spans);
                                        all_lines.push(Line::from(indented));
                                    }
                                }
                            }

                            // Add spacing between blocks
                            if idx + 1 < content.len() {
                                all_lines.push(Line::from(""));
                            }
                        }
                        AssistantContent::ToolCall { .. } => {
                            // Tool calls are rendered as separate Tool messages
                            continue;
                        }
                        AssistantContent::Thought { thought } => {
                            // Render thought with italic style
                            let thought_text = thought.display_text();

                            // Parse markdown for the thought
                            let markdown_styles =
                                super::markdown::MarkdownStyles::from_theme(self.theme);
                            let markdown_text = super::markdown::from_str_with_width(
                                &thought_text,
                                &markdown_styles,
                                self.theme,
                                Some(max_width.saturating_sub(3) as u16),
                            );

                            // Process each line
                            for marked_line in markdown_text.lines {
                                // Check if this line should be wrapped
                                let lines_to_render = if marked_line.no_wrap {
                                    // Don't wrap code block lines
                                    vec![marked_line.line]
                                } else {
                                    // Wrap with reduced width for assistant indent (3 for indent)
                                    style_wrap(marked_line.line, max_width.saturating_sub(3) as u16)
                                };

                                for line in lines_to_render {
                                    let mut styled_spans = Vec::new();

                                    // Apply italic style to all spans in the thought
                                    for span in line.spans {
                                        styled_spans.push(Span::styled(
                                            span.content.into_owned(),
                                            self.theme.style(Component::ThoughtText),
                                        ));
                                    }

                                    if !first_content_rendered {
                                        // First line gets indented gutter
                                        let mut first_line = vec![Span::raw("  ")]; // Indent
                                        first_line
                                            .extend(self.render_gutter(row, is_hovered).spans);
                                        first_line.push(Span::raw(" "));
                                        first_line.extend(styled_spans);
                                        all_lines.push(Line::from(first_line));
                                        first_content_rendered = true;
                                    } else {
                                        // Continuation lines with more indent
                                        let mut indented = vec![Span::raw("    ")]; // 4 spaces
                                        indented.extend(styled_spans);
                                        all_lines.push(Line::from(indented));
                                    }
                                }
                            }

                            // Add spacing between blocks
                            if idx + 1 < content.len() {
                                all_lines.push(Line::from(""));
                            }
                        }
                    }
                }

                // If no content was rendered, just show the gutter
                if !first_content_rendered {
                    let mut glyph_line = vec![Span::raw("  ")]; // Indent
                    glyph_line.extend(self.render_gutter(row, is_hovered).spans);
                    all_lines.push(Line::from(glyph_line));
                }
            }
            Message::Tool {
                tool_use_id,
                result,
                ..
            } => {
                // Find the corresponding tool call
                let tool_call = self.items.iter().find_map(|item| {
                    if let ChatItem::Message(msg_row) = item {
                        if let Message::Assistant { content, .. } = &msg_row.inner {
                            content.iter().find_map(|block| {
                                if let AssistantContent::ToolCall { tool_call } = block {
                                    if tool_call.id == *tool_use_id {
                                        Some(tool_call)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            })
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });

                if let Some(tool_call) = tool_call {
                    // Use the formatter to render the tool
                    let formatter = formatters::get_formatter(&tool_call.name);
                    let wrap_width = (width.saturating_sub(7)) as usize; // Account for indent and padding

                    let formatted_lines = match view_mode {
                        ViewMode::Compact => formatter.compact(
                            &tool_call.parameters,
                            &Some(result.clone()),
                            wrap_width,
                            self.theme,
                        ),
                        ViewMode::Detailed => formatter.detailed(
                            &tool_call.parameters,
                            &Some(result.clone()),
                            wrap_width,
                            self.theme,
                        ),
                    };

                    // First line with indented gutter and tool name
                    let mut first_line = vec![Span::raw("  ")]; // Indent
                    first_line.extend(self.render_gutter(row, is_hovered).spans);
                    first_line.push(Span::raw(" "));
                    first_line.push(Span::styled(
                        tool_call.name.clone(),
                        self.theme.style(Component::ToolCallHeader),
                    ));

                    // For compact mode, try to fit first output on same line
                    if view_mode == ViewMode::Compact && !formatted_lines.is_empty() {
                        let first_output = &formatted_lines[0];
                        if first_output.width()
                            < (wrap_width.saturating_sub(tool_call.name.len() + 5))
                        {
                            // Add status indicator
                            match result {
                                ToolResult::Error(_) => {
                                    first_line.push(Span::styled(
                                        " ✗ ",
                                        self.theme.style(Component::ErrorText),
                                    ));
                                }
                                _ => {
                                    first_line.push(Span::styled(
                                        " ✓ ",
                                        self.theme.style(Component::ToolSuccess),
                                    ));
                                }
                            }
                            first_line.extend(first_output.spans.clone());
                            all_lines.push(Line::from(first_line));

                            // Add remaining lines
                            for line in formatted_lines.iter().skip(1) {
                                let mut indented_spans = vec![Span::raw("    ")]; // Align with content
                                indented_spans.extend(line.spans.clone());
                                all_lines.push(Line::from(indented_spans));
                            }
                        } else {
                            // Add status indicator on first line
                            match result {
                                ToolResult::Error(_) => {
                                    first_line.push(Span::styled(
                                        " ✗",
                                        self.theme.style(Component::ErrorText),
                                    ));
                                }
                                _ => {
                                    first_line.push(Span::styled(
                                        " ✓",
                                        self.theme.style(Component::ToolSuccess),
                                    ));
                                }
                            }
                            all_lines.push(Line::from(first_line));

                            // Add all lines with indent
                            for line in formatted_lines {
                                let mut indented_spans = vec![Span::raw("    ")]; // Align with content
                                indented_spans.extend(line.spans.clone());
                                all_lines.push(Line::from(indented_spans));
                            }
                        }
                    } else {
                        all_lines.push(Line::from(first_line));
                        // Add the formatted output with proper indentation
                        for line in formatted_lines {
                            let mut indented_spans = vec![Span::raw("    ")]; // Align with content
                            indented_spans.extend(line.spans.clone());
                            all_lines.push(Line::from(indented_spans));
                        }
                    }
                } else {
                    // Fallback if we can't find the tool call
                    let mut fallback_line = vec![Span::raw("  ")]; // Indent
                    fallback_line.extend(self.render_gutter(row, is_hovered).spans);
                    fallback_line.push(Span::raw(" "));
                    fallback_line.push(Span::styled(
                        "Tool Result",
                        self.theme.style(Component::ToolCallHeader),
                    ));
                    all_lines.push(Line::from(fallback_line));

                    // Show a simple message for the fallback case
                    let component = match result {
                        ToolResult::Error(_) => Component::ToolError,
                        _ => Component::DimText,
                    };

                    all_lines.push(Line::from(vec![
                        Span::raw("    "),
                        Span::styled(
                            "(Result display unavailable)",
                            self.theme.style(component).add_modifier(Modifier::ITALIC),
                        ),
                    ]));
                }
            }
        }

        let height = all_lines.len() as u16;

        RenderCache {
            lines: all_lines,
            height,
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

        // Calculate total height and item positions, managing cache
        let mut item_positions = Vec::new();
        let mut total_height = 0u16;

        for (idx, item) in self.items.iter().enumerate() {
            let (_, height) = self.render_chat_items(
                item,
                list_area.width,
                state.view_mode,
                Some(&mut state.line_cache),
            );

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

                // Get lines for the item
                let (lines, _) = self.render_chat_items(
                    item,
                    list_area.width,
                    state.view_mode,
                    Some(&mut state.line_cache),
                );
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
