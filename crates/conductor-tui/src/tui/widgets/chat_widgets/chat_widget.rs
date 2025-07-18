//! Chat widget trait and implementations for bounded rendering
//!
//! This module provides the core abstraction for rendering chat items
//! within precise rectangular bounds, preventing buffer overlap issues.

use crate::tui::model::MessageRow;
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::message_widget::MessageWidget;
use conductor_core::app::conversation::{AssistantContent, Message};
use conductor_tools::{ToolCall, ToolResult};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

/// Core trait for chat items that can compute their height and render themselves
pub trait ChatWidget: Send + Sync {
    /// Compute height (rows).
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize;

    /// Render full widget.
    fn render(&mut self, area: Rect, buf: &mut Buffer, mode: ViewMode, theme: &Theme);

    /// Render starting from `first_line` of logical output.
    /// Default implementation draws to an off-screen buffer then copies the
    /// requested slice – this preserves behaviour without per-widget work.
    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    );
}

/// Height cache for efficient scrolling
#[derive(Debug, Clone)]
pub struct HeightCache {
    pub compact: Option<usize>,
    pub detailed: Option<usize>,
    pub last_width: u16,
}

impl Default for HeightCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HeightCache {
    pub fn new() -> Self {
        Self {
            compact: None,
            detailed: None,
            last_width: 0,
        }
    }

    pub fn invalidate(&mut self) {
        self.compact = None;
        self.detailed = None;
        self.last_width = 0;
    }

    pub fn get(&self, mode: ViewMode, width: u16) -> Option<usize> {
        if self.last_width != width {
            return None;
        }
        match mode {
            ViewMode::Compact => self.compact,
            ViewMode::Detailed => self.detailed,
        }
    }

    pub fn set(&mut self, mode: ViewMode, width: u16, height: usize) {
        self.last_width = width;
        match mode {
            ViewMode::Compact => self.compact = Some(height),
            ViewMode::Detailed => self.detailed = Some(height),
        }
    }
}

/// Default widget that renders simple text as a paragraph
pub struct ParagraphWidget {
    lines: Vec<Line<'static>>,
    cache: HeightCache,
}

impl ParagraphWidget {
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        Self {
            lines,
            cache: HeightCache::new(),
        }
    }

    pub fn from_text(text: String, theme: &Theme) -> Self {
        Self::from_styled_text(text, theme.style(Component::NoticeInfo))
    }

    pub fn from_styled_text(text: String, style: ratatui::style::Style) -> Self {
        let lines = text
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), style)))
            .collect();
        Self::new(lines)
    }

    /// Create a paragraph with theme background applied
    fn create_themed_paragraph(&self, theme: &Theme) -> Paragraph<'static> {
        let bg_style = theme.style(Component::ChatListBackground);
        Paragraph::new(self.lines.clone())
            .wrap(Wrap { trim: false })
            .style(bg_style)
    }
}

impl ChatWidget for ParagraphWidget {
    fn height(&mut self, mode: ViewMode, width: u16, _theme: &Theme) -> usize {
        // Check cache first
        if let Some(cached) = self.cache.get(mode, width) {
            return cached;
        }

        // Calculate height based on line wrapping
        let mut total_height = 0;
        for line in &self.lines {
            let line_width = line.width();
            if line_width <= width as usize {
                total_height += 1;
            } else {
                // Simple wrapping calculation
                total_height += line_width.div_ceil(width as usize);
            }
        }

        // Cache the result
        self.cache.set(mode, width, total_height);
        total_height
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, _mode: ViewMode, theme: &Theme) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Create a paragraph with the lines and theme background
        let paragraph = self.create_themed_paragraph(theme);

        // Render into the exact area
        paragraph.render(area, buf);
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        _mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        // Ensure we have lines to render
        if first_line >= self.lines.len() {
            return;
        }

        // Calculate the slice of lines to render
        let end_line = (first_line + area.height as usize).min(self.lines.len());
        let visible_lines = &self.lines[first_line..end_line];

        // Create a paragraph with only the visible lines
        let bg_style = theme.style(Component::ChatListBackground);
        let paragraph = Paragraph::new(visible_lines.to_vec())
            .wrap(Wrap { trim: false })
            .style(bg_style);

        // Render into the exact area
        paragraph.render(area, buf);
    }
}

/// Represents different types of chat blocks that can be rendered
#[derive(Debug, Clone)]
pub enum ChatBlock {
    /// A message from user or assistant
    Message(Message),
    /// A tool call and its result (coupled together)
    ToolInteraction {
        call: ToolCall,
        result: Option<ToolResult>,
    },
}

impl ChatBlock {
    /// Create ChatBlocks from a MessageRow, handling coupled tool calls and results
    pub fn from_message_row(row: &MessageRow, all_messages: &[&MessageRow]) -> Vec<ChatBlock> {
        match &row.inner {
            Message::User { .. } => {
                vec![ChatBlock::Message(row.inner.clone())]
            }
            Message::Assistant { content, .. } => {
                let mut blocks = vec![];
                let mut has_text = false;
                let mut tool_calls = vec![];

                // Separate text content from tool calls
                for block in content {
                    match block {
                        AssistantContent::Text { text } => {
                            if !text.trim().is_empty() {
                                has_text = true;
                            }
                        }
                        AssistantContent::ToolCall { tool_call } => {
                            tool_calls.push(tool_call.clone());
                        }
                        AssistantContent::Thought { .. } => {
                            has_text = true; // Thoughts count as text content
                        }
                    }
                }

                // Add text message if present
                if has_text {
                    blocks.push(ChatBlock::Message(row.inner.clone()));
                }

                // Add tool interactions (coupled with their results)
                for tool_call in tool_calls {
                    // Find the corresponding tool result
                    let result = all_messages.iter().find_map(|msg_row| {
                        if let Message::Tool {
                            tool_use_id,
                            result,
                            ..
                        } = &msg_row.inner
                        {
                            if tool_use_id == &tool_call.id {
                                Some(result.clone())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });

                    blocks.push(ChatBlock::ToolInteraction {
                        call: tool_call,
                        result,
                    });
                }

                blocks
            }
            Message::Tool {
                tool_use_id,
                result,
                ..
            } => {
                // Check if this tool result has a corresponding tool call in the assistant messages
                let has_corresponding_call = all_messages.iter().any(|msg_row| {
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
                });

                if has_corresponding_call {
                    // This will be rendered as part of the assistant's tool interaction
                    vec![]
                } else {
                    // Standalone tool result - render it
                    vec![ChatBlock::ToolInteraction {
                        call: conductor_tools::ToolCall {
                            id: tool_use_id.clone(),
                            name: "Unknown".to_string(), // We don't have the tool name in standalone results
                            parameters: serde_json::Value::Null,
                        },
                        result: Some(result.clone()),
                    }]
                }
            }
        }
    }
}

/// Dynamic chat widget that can render any ChatBlock
pub struct DynamicChatWidget {
    inner: Box<dyn ChatWidget + Send + Sync>,
}

impl DynamicChatWidget {
    pub fn from_block(block: ChatBlock, _theme: &Theme) -> Self {
        let inner: Box<dyn ChatWidget + Send + Sync> = match block {
            ChatBlock::Message(message) => Box::new(MessageWidget::new(message)),
            ChatBlock::ToolInteraction { call, result } => {
                // Use the ToolFormatterWidget for tool interactions
                Box::new(
                    crate::tui::widgets::chat_widgets::tool_widget::ToolWidget::new(call, result),
                )
            }
        };
        Self { inner }
    }
}

impl ChatWidget for DynamicChatWidget {
    fn height(&mut self, mode: ViewMode, width: u16, theme: &Theme) -> usize {
        self.inner.height(mode, width, theme)
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, mode: ViewMode, theme: &Theme) {
        self.inner.render(area, buf, mode, theme)
    }

    fn render_partial(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        mode: ViewMode,
        theme: &Theme,
        first_line: usize,
    ) {
        self.inner
            .render_partial(area, buf, mode, theme, first_line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;
    use conductor_core::app::conversation::UserContent;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    #[test]
    fn test_paragraph_widget_height_calculation() {
        let theme = Theme::default();
        let mut widget = ParagraphWidget::from_text("Hello world".to_string(), &theme);

        // Test with width that fits
        assert_eq!(widget.height(ViewMode::Compact, 20, &theme), 1);

        // Test with narrow width that requires wrapping
        // "Hello world" is 11 characters, so with width 5, it would need at least 3 lines
        assert_eq!(widget.height(ViewMode::Compact, 5, &theme), 3);

        // Test cache hit
        assert_eq!(widget.height(ViewMode::Compact, 20, &theme), 1);
    }

    #[test]
    fn test_paragraph_widget_stays_in_bounds() {
        let theme = Theme::default();
        let mut widget = ParagraphWidget::from_text(
            "This is a long line that should wrap when rendered in a narrow area".to_string(),
            &theme,
        );

        // Create a test terminal
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = Rect::new(5, 2, 20, 3); // Bounded area
                widget.render(area, f.buffer_mut(), ViewMode::Compact, &theme);
            })
            .unwrap();

        // Check that nothing was rendered outside the bounds
        let buffer = terminal.backend().buffer();

        // Check areas outside the widget bounds are empty
        for y in 0..10 {
            for x in 0..40 {
                if !(2..5).contains(&y) || !(5..25).contains(&x) {
                    // This should be outside our render area
                    let cell = &buffer[(x, y)];
                    assert_eq!(
                        cell.symbol(),
                        " ",
                        "Found content outside bounds at ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn test_height_cache_invalidation() {
        let theme = Theme::default();
        let mut widget = ParagraphWidget::from_text("Test text".to_string(), &theme);

        // Initial calculation
        let h1 = widget.height(ViewMode::Compact, 10, &theme);

        // Should return cached value
        let h2 = widget.height(ViewMode::Compact, 10, &theme);
        assert_eq!(h1, h2);

        // Different width should recalculate
        let h3 = widget.height(ViewMode::Compact, 4, &theme);
        // With width 4, "Test text" (9 chars) needs at least 3 lines
        assert_eq!(h3, 3);

        // Different mode with same width should use separate cache
        let h4 = widget.height(ViewMode::Detailed, 10, &theme);
        assert_eq!(h1, h4); // Same text, so same height
    }

    #[test]
    fn test_chat_block_from_message_row() {
        let user_msg = Message::User {
            content: vec![UserContent::Text {
                text: "Test".to_string(),
            }],
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let row = MessageRow::new(user_msg);
        let blocks = ChatBlock::from_message_row(&row, &[]);

        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::Message(_) => {} // Expected
            _ => panic!("Expected Message ChatBlock"),
        }
    }

    #[test]
    fn test_dynamic_chat_widget() {
        let theme = Theme::default();
        let user_msg = Message::User {
            content: vec![UserContent::Text {
                text: "Test message".to_string(),
            }],
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let block = ChatBlock::Message(user_msg);
        let mut widget = DynamicChatWidget::from_block(block, &theme);

        // Test that it delegates correctly
        let height = widget.height(ViewMode::Compact, 20, &theme);
        assert_eq!(height, 1);
    }
}
