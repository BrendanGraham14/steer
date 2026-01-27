//! Chat widget trait and implementations for bounded rendering
//!
//! This module provides the core abstraction for rendering chat items
//! within precise rectangular bounds, preventing buffer overlap issues.

use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::chat_list_state::ViewMode;
use crate::tui::widgets::chat_widgets::message_widget::MessageWidget;
use ratatui::text::{Line, Span};
use steer_grpc::client_api::{AssistantContent, Message, MessageData, ToolCall, ToolResult};

/// Core trait for chat items that can compute their height and render themselves
pub trait ChatRenderable: Send + Sync {
    /// Return formatted lines; cache internally on `(width, mode)`.
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>];
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
}

impl ParagraphWidget {
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines }
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
}

impl ChatRenderable for ParagraphWidget {
    fn lines(&mut self, _width: u16, _mode: ViewMode, _theme: &Theme) -> &[Line<'static>] {
        &self.lines
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
    pub fn from_message_row(message: &Message, all_messages: &[&Message]) -> Vec<ChatBlock> {
        match &message.data {
            MessageData::User { .. } => {
                vec![ChatBlock::Message(message.clone())]
            }
            MessageData::Assistant { content, .. } => {
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
                        AssistantContent::ToolCall { tool_call, .. } => {
                            tool_calls.push(tool_call.clone());
                        }
                        AssistantContent::Thought { .. } => {
                            has_text = true; // Thoughts count as text content
                        }
                    }
                }

                // Add text message if present
                if has_text {
                    blocks.push(ChatBlock::Message(message.clone()));
                }

                // Add tool interactions (coupled with their results)
                for tool_call in tool_calls {
                    // Find the corresponding tool result
                    let result = all_messages.iter().find_map(|msg_row| {
                        if let MessageData::Tool {
                            tool_use_id,
                            result,
                            ..
                        } = &msg_row.data
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
            MessageData::Tool {
                tool_use_id,
                result,
                ..
            } => {
                // Check if this tool result has a corresponding tool call in the assistant messages
                let has_corresponding_call = all_messages.iter().any(|msg_row| {
                    if let MessageData::Assistant { content, .. } = &msg_row.data {
                        content.iter().any(|block| {
                            if let AssistantContent::ToolCall { tool_call, .. } = block {
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
                        call: steer_tools::ToolCall {
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
    inner: Box<dyn ChatRenderable + Send + Sync>,
}

impl DynamicChatWidget {
    pub fn from_block(block: ChatBlock, _theme: &Theme) -> Self {
        let inner: Box<dyn ChatRenderable + Send + Sync> = match block {
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

impl ChatRenderable for DynamicChatWidget {
    fn lines(&mut self, width: u16, mode: ViewMode, theme: &Theme) -> &[Line<'static>] {
        self.inner.lines(width, mode, theme)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::Theme;
    use steer_grpc::client_api::UserContent;

    #[test]
    fn test_chat_block_from_message_row() {
        let user_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Test".to_string(),
                }],
            },
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let blocks = ChatBlock::from_message_row(&user_msg, &[]);

        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::Message(_) => {} // Expected
            _ => panic!("Expected Message ChatBlock"),
        }
    }

    #[test]
    fn test_dynamic_chat_widget() {
        let theme = Theme::default();
        let user_msg = Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Test message".to_string(),
                }],
            },
            timestamp: 0,
            id: "test-id".to_string(),
            parent_message_id: None,
        };

        let block = ChatBlock::Message(user_msg);
        let mut widget = DynamicChatWidget::from_block(block, &theme);

        // Test that it delegates correctly
        let height = widget.lines(20, ViewMode::Compact, &theme).len();
        assert_eq!(height, 1);
    }
}
