use crate::app::conversation::{AssistantContent, Message, ToolResult, UserContent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{
        Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
    },
};
use std::collections::HashSet;
use tools::ToolCall;
use tui_widgets::scrollview::{ScrollView, ScrollViewState, ScrollbarVisibility};

use super::content_renderer::{ContentRenderer, DefaultContentRenderer};
use super::styles;

/// Pure data model for messages
#[derive(Debug, Clone)]
pub enum MessageContent {
    User {
        id: String,
        blocks: Vec<UserContent>,
        timestamp: String,
    },
    Assistant {
        id: String,
        blocks: Vec<AssistantContent>,
        timestamp: String,
    },
    Tool {
        id: String,
        call: ToolCall,
        result: Option<ToolResult>,
        timestamp: String,
    },
}

impl MessageContent {
    pub fn id(&self) -> &str {
        match self {
            Self::User { id, .. } => id,
            Self::Assistant { id, .. } => id,
            Self::Tool { id, .. } => id,
        }
    }

    // Note: Message to MessageContent conversion should be done through the TUI's
    // convert_message functions which have access to the tool registry. This is
    // necessary because tool calls are embedded in assistant messages and need
    // to be tracked separately for proper matching with tool results.
}

/// View modes for different detail levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewMode {
    Compact,
    Detailed,
}

/// View preferences per content type
#[derive(Debug, Clone)]
pub struct ViewPreferences {
    pub tool_calls: ViewMode,
    pub tool_results: ViewMode,
    pub thoughts: ViewMode,
    pub text: ViewMode,
    pub command_execution: ViewMode,
    pub global_override: Option<ViewMode>,
}

impl Default for ViewPreferences {
    fn default() -> Self {
        Self {
            tool_calls: ViewMode::Compact,
            tool_results: ViewMode::Compact,
            thoughts: ViewMode::Detailed,
            text: ViewMode::Detailed,
            command_execution: ViewMode::Compact,
            global_override: None,
        }
    }
}

/// State for the message list widget
#[derive(Debug, Clone)]
pub struct MessageListState {
    pub expanded_messages: HashSet<String>,
    pub selected: Option<usize>,
    pub view_prefs: ViewPreferences,
    /// Track if user has manually scrolled away from bottom
    pub user_scrolled: bool,

    // ScrollView state for handling scrolling
    pub scroll_state: ScrollViewState,
}

impl MessageListState {
    pub fn new() -> Self {
        Self {
            expanded_messages: HashSet::new(),
            selected: None,
            view_prefs: ViewPreferences::default(),
            user_scrolled: false,
            scroll_state: ScrollViewState::default(),
        }
    }

    pub fn toggle_expanded(&mut self, message_id: String) {
        if self.expanded_messages.contains(&message_id) {
            self.expanded_messages.remove(&message_id);
        } else {
            self.expanded_messages.insert(message_id);
        }
    }

    pub fn is_expanded(&self, message_id: &str) -> bool {
        self.expanded_messages.contains(message_id)
    }

    pub fn select_next(&mut self, total_messages: usize) {
        if total_messages == 0 {
            return;
        }

        self.selected = match self.selected {
            Some(idx) if idx < total_messages - 1 => Some(idx + 1),
            Some(_) => Some(total_messages - 1),
            None => Some(0),
        };
    }

    pub fn select_previous(&mut self) {
        self.selected = match self.selected {
            Some(idx) if idx > 0 => Some(idx - 1),
            Some(_) => Some(0),
            None => None,
        };
    }

    pub fn scroll_to_selected(
        &mut self,
        messages: &[MessageContent],
        renderer: &dyn ContentRenderer,
        width: u16,
    ) {
        if let Some(selected) = self.selected {
            let mut y_offset = 0u16;
            for (idx, message) in messages.iter().enumerate() {
                if idx == selected {
                    // Just set the offset to show this message at the top
                    self.scroll_state.set_offset((0, y_offset).into());
                    self.user_scrolled = true;
                    break;
                }

                let mode = if self.view_prefs.global_override.is_some() {
                    self.view_prefs.global_override.unwrap()
                } else {
                    match message {
                        MessageContent::Tool { .. } => self.view_prefs.tool_calls,
                        MessageContent::Assistant { blocks, .. } => {
                            if blocks
                                .iter()
                                .any(|b| matches!(b, AssistantContent::Thought { .. }))
                            {
                                ViewMode::Detailed
                            } else {
                                self.view_prefs.text
                            }
                        }
                        _ => self.view_prefs.text,
                    }
                };

                let height = renderer.calculate_height(message, mode, width);
                y_offset = y_offset.saturating_add(height);
                if idx + 1 < messages.len() {
                    y_offset = y_offset.saturating_add(1); // Gap between messages
                }
            }
        }
    }

    /// Check if the view is at or near the bottom
    pub fn is_at_bottom(&self) -> bool {
        // Since scroll_to_bottom sets offset to u16::MAX and the renderer adjusts it,
        // we can't reliably check if we're at the bottom based on offset alone.
        // Instead, we'll track this with the user_scrolled flag.
        !self.user_scrolled
    }

    /// Reset to bottom and clear manual scroll flag
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_state.scroll_to_bottom();
        self.user_scrolled = false;
    }

    /// Set the scroll offset and update user_scrolled flag
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_state.set_offset((0, offset as u16).into());
        self.user_scrolled = true;
    }
}

/// Main widget for rendering a list of messages
pub struct MessageList<'a> {
    messages: &'a [MessageContent],
    renderer: Box<dyn ContentRenderer>,
    block: Option<Block<'a>>,
}

impl<'a> MessageList<'a> {
    pub fn new(messages: &'a [MessageContent]) -> Self {
        Self {
            messages,
            renderer: Box::new(DefaultContentRenderer),
            block: Some(Block::default().borders(Borders::ALL)),
        }
    }

    pub fn with_renderer(mut self, renderer: Box<dyn ContentRenderer>) -> Self {
        self.renderer = renderer;
        self
    }

    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    fn determine_view_mode(&self, content: &MessageContent, state: &MessageListState) -> ViewMode {
        // Check for global override first
        if let Some(mode) = state.view_prefs.global_override {
            return mode;
        }

        // Determine mode based on content type and preferences
        match content {
            MessageContent::Tool { .. } => state.view_prefs.tool_calls,
            MessageContent::Assistant { blocks, .. } => {
                // Always show thoughts in detailed mode, regardless of view preferences
                // Only tool blocks within assistant messages should be affected by view mode
                if blocks
                    .iter()
                    .any(|b| matches!(b, AssistantContent::Thought { .. }))
                {
                    ViewMode::Detailed
                } else {
                    state.view_prefs.text
                }
            }
            _ => state.view_prefs.text,
        }
    }

    fn calculate_total_height(&self, width: u16, state: &MessageListState) -> u16 {
        let mut total = 0u16;
        for (idx, msg) in self.messages.iter().enumerate() {
            let mode = self.determine_view_mode(msg, state);
            total = total.saturating_add(self.renderer.calculate_height(msg, mode, width));

            // Add gap between messages (but not after the last one)
            if idx + 1 < self.messages.len() {
                total = total.saturating_add(1);
            }
        }
        total
    }
}

impl<'a> StatefulWidget for MessageList<'a> {
    type State = MessageListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Render block if provided
        let messages_area = if let Some(block) = &self.block {
            let inner = block.inner(area);
            block.render(area, buf);
            inner
        } else {
            area
        };

        // Calculate total content size
        let total_height = self.calculate_total_height(messages_area.width, state);
        let content_size = ratatui::layout::Size {
            width: messages_area.width,
            height: total_height,
        };

        // The scroll view will update the state automatically during render

        // Create ScrollView and render messages into it
        let mut scroll_view = ScrollView::new(content_size)
            .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);

        // Create a custom widget that renders all messages
        let messages_widget = MessagesRenderer {
            messages: self.messages,
            renderer: &*self.renderer,
            state,
        };

        // Render the messages widget into the scroll view
        scroll_view.render_widget(
            messages_widget,
            Rect::new(0, 0, content_size.width, content_size.height),
        );

        // Render the scroll view to the screen
        scroll_view.render(messages_area, buf, &mut state.scroll_state);

        // Render scrollbar if needed
        if total_height > messages_area.height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(total_height as usize)
                .position(state.scroll_state.offset().y as usize);

            scrollbar.render(messages_area, buf, &mut scrollbar_state);
        }
    }
}

// Internal widget for rendering messages within the ScrollView
struct MessagesRenderer<'a> {
    messages: &'a [MessageContent],
    renderer: &'a dyn ContentRenderer,
    state: &'a MessageListState,
}

impl<'a> Widget for MessagesRenderer<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut y = 0;

        for (idx, message) in self.messages.iter().enumerate() {
            let mode = self.determine_view_mode(message);
            let height = self.renderer.calculate_height(message, mode, area.width);

            // Check if this message is visible in the current area
            if y + height < area.y {
                // Message is above visible area, skip rendering but count height
                y += height;
                if idx + 1 < self.messages.len() {
                    y += 1; // Gap between messages
                }
                continue;
            }

            if y >= area.y + area.height {
                // Message is below visible area, stop rendering
                break;
            }

            // Calculate the actual area for this message (might be clipped)
            let message_y = y.max(area.y);
            let visible_height = (y + height).min(area.y + area.height) - message_y;

            let message_area = Rect {
                x: area.x,
                y: message_y,
                width: area.width,
                height: visible_height,
            };

            // Highlight selected message
            if self.state.selected == Some(idx) {
                let highlight_block = Block::default()
                    .borders(Borders::LEFT)
                    .border_style(styles::SELECTION_HIGHLIGHT);
                highlight_block.render(message_area, buf);

                // Adjust area for content
                let content_area = Rect {
                    x: message_area.x + 1,
                    width: message_area.width.saturating_sub(1),
                    ..message_area
                };

                self.renderer.render(message, mode, content_area, buf);
            } else {
                self.renderer.render(message, mode, message_area, buf);
            }

            y += height;

            // Add gap between messages
            if idx + 1 < self.messages.len() {
                y += 1;
            }
        }
    }
}

impl<'a> MessagesRenderer<'a> {
    fn determine_view_mode(&self, content: &MessageContent) -> ViewMode {
        // Check for global override first
        if let Some(mode) = self.state.view_prefs.global_override {
            return mode;
        }

        // Determine mode based on content type and preferences
        match content {
            MessageContent::Tool { .. } => self.state.view_prefs.tool_calls,
            MessageContent::Assistant { blocks, .. } => {
                // Always show thoughts in detailed mode, regardless of view preferences
                if blocks
                    .iter()
                    .any(|b| matches!(b, AssistantContent::Thought { .. }))
                {
                    ViewMode::Detailed
                } else {
                    self.state.view_prefs.text
                }
            }
            _ => self.state.view_prefs.text,
        }
    }
}

// For backwards compatibility, provide a type alias
pub type MessageViewState = MessageListState;
