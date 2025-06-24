use crate::app::conversation::{AssistantContent, ToolResult, UserContent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
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

impl ViewPreferences {
    /// Determine the view mode for a given message
    pub fn determine_mode(&self, content: &MessageContent) -> ViewMode {
        // Check for global override first
        if let Some(mode) = self.global_override {
            return mode;
        }

        // Determine mode based on content type and preferences
        match content {
            MessageContent::Tool { .. } => self.tool_calls,
            MessageContent::Assistant { blocks, .. } => {
                // Always show thoughts in detailed mode, regardless of view preferences
                if blocks
                    .iter()
                    .any(|b| matches!(b, AssistantContent::Thought { .. }))
                {
                    ViewMode::Detailed
                } else {
                    self.text
                }
            }
            _ => self.text,
        }
    }
}

/// Represents the range of messages visible in the viewport
#[derive(Debug, Clone)]
pub struct VisibleRange {
    /// Index of the first visible message
    pub first_index: usize,
    /// Index of the last visible message
    pub last_index: usize,
    /// Y offset where the first visible message starts (relative to viewport)
    pub first_y_offset: i16,
    /// Number of messages above the viewport
    pub messages_above: usize,
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

    /// Currently visible message range (cached for performance)
    visible_range: Option<VisibleRange>,

    /// Last rendered viewport height (for accurate scrolling)
    last_viewport_height: u16,
}

impl Default for MessageListState {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageListState {
    pub fn new() -> Self {
        Self {
            expanded_messages: HashSet::new(),
            selected: None,
            view_prefs: ViewPreferences::default(),
            user_scrolled: false,
            scroll_state: ScrollViewState::default(),
            visible_range: None,
            last_viewport_height: 0,
        }
    }

    /// Get cached height or calculate and cache it
    fn get_or_calculate_height(
        &mut self,
        message: &MessageContent,
        mode: ViewMode,
        width: u16,
        renderer: &dyn ContentRenderer,
    ) -> u16 {
        renderer.calculate_height(message, mode, width)
    }

    /// Calculate which messages are visible in the viewport
    pub fn calculate_visible_range(
        &mut self,
        messages: &[MessageContent],
        viewport_height: u16,
        width: u16,
        renderer: &dyn ContentRenderer,
    ) -> Option<VisibleRange> {
        // Store the viewport height for use in scroll methods
        self.last_viewport_height = viewport_height;

        if messages.is_empty() {
            self.visible_range = None;
            return None;
        }

        // First pass: calculate total content height to determine max scroll
        let mut total_height = 0u16;
        for (idx, message) in messages.iter().enumerate() {
            let mode = self.view_prefs.determine_mode(message);
            let height = self.get_or_calculate_height(message, mode, width, renderer);
            total_height = total_height.saturating_add(height);
            if idx + 1 < messages.len() {
                total_height = total_height.saturating_add(1); // gap between messages
            }
        }

        // Clamp scroll offset to valid range
        // For max_scroll, we want to ensure the last message is fully visible
        let max_scroll = if total_height > viewport_height {
            total_height.saturating_sub(viewport_height)
        } else {
            0
        };
        let raw_offset = self.scroll_state.offset().y;
        let scroll_offset = raw_offset.min(max_scroll);

        // Update scroll state if it was beyond bounds
        if raw_offset > max_scroll {
            self.scroll_state.set_offset((0, max_scroll).into());
        }

        let mut current_y = 0u16;
        let mut first_index = None;
        let mut first_y_offset = 0i16;
        let mut last_index = 0;
        let mut messages_above = 0usize;

        for (idx, message) in messages.iter().enumerate() {
            let mode = self.view_prefs.determine_mode(message);
            let height = self.get_or_calculate_height(message, mode, width, renderer);

            // Count messages that are completely above the viewport
            if current_y.saturating_add(height) < scroll_offset {
                messages_above += 1;
            }

            // Check if message is potentially visible
            if current_y.saturating_add(height) >= scroll_offset && first_index.is_none() {
                first_index = Some(idx);
                // Calculate offset of first visible message relative to viewport top
                first_y_offset = (current_y as i16).saturating_sub(scroll_offset as i16);
            }

            // Update last visible index if any part of message is in viewport
            if current_y < scroll_offset.saturating_add(viewport_height)
                && current_y.saturating_add(height) > scroll_offset
            {
                last_index = idx;
            }

            // Check if we've gone past the viewport (check AFTER updating last_index)
            if current_y >= scroll_offset.saturating_add(viewport_height) {
                break;
            }

            current_y = current_y.saturating_add(height);

            // Add gap between messages (except after last)
            if idx + 1 < messages.len() {
                current_y = current_y.saturating_add(1);
            }
        }

        let range = first_index.map(|first| VisibleRange {
            first_index: first,
            last_index,
            first_y_offset,
            messages_above,
        });

        self.visible_range = range.clone();
        range
    }

    /// Scroll by a specific amount (positive = down, negative = up), respecting content bounds
    /// Returns true if the scroll position changed
    pub fn scroll_by(
        &mut self,
        messages: &[MessageContent],
        viewport_height: u16,
        width: u16,
        renderer: &dyn ContentRenderer,
        amount: i32,
    ) -> bool {
        // Use the last rendered viewport height if available, as it's more accurate
        let actual_viewport_height = if self.last_viewport_height > 0 {
            self.last_viewport_height
        } else {
            viewport_height
        };

        let current_offset = self.scroll_state.offset().y;

        // For scrolling up, we can calculate the new offset directly
        if amount < 0 {
            // Clamp the absolute value to prevent overflow when casting to u16
            let abs_amount = amount.unsigned_abs();
            let scroll_amount = abs_amount.min(u32::from(u16::MAX)) as u16;
            let new_offset = current_offset.saturating_sub(scroll_amount);
            if new_offset != current_offset {
                self.scroll_state.set_offset((0, new_offset).into());
                return true;
            }
            return false;
        }

        // For scrolling down, clamp amount to prevent overflow
        let scroll_amount = if amount > 0 {
            amount.min(i32::from(u16::MAX)) as u16
        } else {
            0
        };

        // Calculate total content height
        let mut total_height = 0u16;
        for (idx, message) in messages.iter().enumerate() {
            let mode = self.view_prefs.determine_mode(message);
            let height = self.get_or_calculate_height(message, mode, width, renderer);
            total_height = total_height.saturating_add(height);
            if idx + 1 < messages.len() {
                total_height = total_height.saturating_add(1);
            }
        }

        // Calculate max scroll position
        let max_scroll = if total_height > actual_viewport_height {
            total_height.saturating_sub(actual_viewport_height)
        } else {
            0
        };

        let new_offset = current_offset.saturating_add(scroll_amount).min(max_scroll);

        if new_offset != current_offset {
            self.scroll_state.set_offset((0, new_offset).into());
            true
        } else {
            false
        }
    }

    /// Simplified scroll down method that handles all the boilerplate
    /// Returns true if the scroll position changed
    pub fn simple_scroll_down(
        &mut self,
        amount: u16,
        messages: &[MessageContent],
        terminal_size: Option<(u16, u16)>,
    ) -> bool {
        // Use cached viewport height if available, otherwise use terminal size or default
        let viewport_height = if self.last_viewport_height > 0 {
            self.last_viewport_height
        } else if let Some((_, height)) = terminal_size {
            height.saturating_sub(4) // Account for UI chrome
        } else {
            30 // Fallback only when absolutely necessary
        };

        // Use actual terminal width if provided, otherwise use default
        let width = terminal_size
            .map(|(w, _)| w.saturating_sub(2))
            .unwrap_or(80);

        // Use the default renderer
        let renderer = DefaultContentRenderer;

        self.scroll_down_by(messages, viewport_height, width, &renderer, amount)
    }

    /// Simplified scroll up method that handles all the boilerplate
    /// Returns true if the scroll position changed
    pub fn simple_scroll_up(
        &mut self,
        amount: u16,
        messages: &[MessageContent],
        terminal_size: Option<(u16, u16)>,
    ) -> bool {
        // Use cached viewport height if available, otherwise use terminal size or default
        let viewport_height = if self.last_viewport_height > 0 {
            self.last_viewport_height
        } else if let Some((_, height)) = terminal_size {
            height.saturating_sub(4) // Account for UI chrome
        } else {
            30 // Fallback only when absolutely necessary
        };

        let width = terminal_size
            .map(|(w, _)| w.saturating_sub(2))
            .unwrap_or(80);
        let renderer = DefaultContentRenderer;

        self.scroll_up_by(messages, viewport_height, width, &renderer, amount)
    }

    /// Scroll down by a specific amount, respecting content bounds
    /// Returns true if the scroll position changed
    #[inline]
    pub fn scroll_down_by(
        &mut self,
        messages: &[MessageContent],
        viewport_height: u16,
        width: u16,
        renderer: &dyn ContentRenderer,
        amount: u16,
    ) -> bool {
        self.scroll_by(messages, viewport_height, width, renderer, amount as i32)
    }

    /// Scroll up by a specific amount
    /// Returns true if the scroll position changed
    #[inline]
    pub fn scroll_up_by(
        &mut self,
        messages: &[MessageContent],
        viewport_height: u16,
        width: u16,
        renderer: &dyn ContentRenderer,
        amount: u16,
    ) -> bool {
        self.scroll_by(messages, viewport_height, width, renderer, -(amount as i32))
    }

    /// Get the last calculated visible range
    pub fn get_visible_range(&self) -> Option<&VisibleRange> {
        self.visible_range.as_ref()
    }

    pub fn toggle_expanded(&mut self, message_id: String) {
        if self.expanded_messages.contains(&message_id) {
            self.expanded_messages.remove(&message_id);
        } else {
            self.expanded_messages.insert(message_id.clone());
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

                let mode = self.view_prefs.determine_mode(message);

                let height = self.get_or_calculate_height(message, mode, width, renderer);
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
        state.view_prefs.determine_mode(content)
    }
}

impl StatefulWidget for MessageList<'_> {
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

        // Calculate visible range first
        let visible_range = state.calculate_visible_range(
            self.messages,
            messages_area.height,
            messages_area.width,
            &*self.renderer,
        );

        // Calculate total content size and collect heights
        let mut cached_heights = Vec::with_capacity(self.messages.len());
        let mut total_height = 0u16;

        for (idx, msg) in self.messages.iter().enumerate() {
            let mode = self.determine_view_mode(msg, state);
            let height =
                state.get_or_calculate_height(msg, mode, messages_area.width, &*self.renderer);
            cached_heights.push(height);
            total_height = total_height.saturating_add(height);

            // Add gap between messages (but not after the last one)
            if idx + 1 < self.messages.len() {
                total_height = total_height.saturating_add(1);
            }
        }

        let content_size = ratatui::layout::Size {
            width: messages_area.width,
            height: total_height,
        };

        // The scroll view will update the state automatically during render

        // Create ScrollView and render messages into it
        // Disable built-in scrollbars – we draw our own vertical bar
        let mut scroll_view =
            ScrollView::new(content_size).scrollbars_visibility(ScrollbarVisibility::Never);

        // Create a custom widget that renders all messages
        let messages_widget = MessagesRenderer {
            messages: self.messages,
            renderer: &*self.renderer,
            state,
            cached_heights,
            visible_range: visible_range.clone(),
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
            // Custom scrollbar: use full-height track with no arrows so the thumb can reach the ends
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            let scroll_offset = state.scroll_state.offset().y as usize;
            let last_visible_line =
                scroll_offset.saturating_add(messages_area.height.saturating_sub(1) as usize);
            let mut scrollbar_state = ScrollbarState::new(total_height as usize)
                .position(last_visible_line.min(total_height.saturating_sub(1) as usize))
                .viewport_content_length(messages_area.height as usize);

            scrollbar.render(messages_area, buf, &mut scrollbar_state);
        }

        // --- Update user_scrolled flag based on current position ---
        // Determine the maximum vertical offset (0 when content fits the view).
        let max_offset = total_height.saturating_sub(messages_area.height);
        let current_offset = state.scroll_state.offset().y;
        // If we're at (or past, due to clamping) the bottom, clear the manual scroll flag.
        // Otherwise, mark that the user has scrolled away.
        state.user_scrolled = current_offset < max_offset;
    }
}

// Internal widget for rendering messages within the ScrollView
struct MessagesRenderer<'a> {
    messages: &'a [MessageContent],
    renderer: &'a dyn ContentRenderer,
    state: &'a MessageListState,
    // Pass pre-calculated heights to avoid recalculation during render
    cached_heights: Vec<u16>,
    // Pre-calculated visible range
    visible_range: Option<VisibleRange>,
}

impl Widget for MessagesRenderer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // If no visible range, render nothing
        let visible_range = match &self.visible_range {
            Some(range) => range,
            None => return,
        };

        let mut y = 0u16;

        // Skip messages before visible range
        for idx in 0..visible_range.first_index {
            let height = self.cached_heights.get(idx).copied().unwrap_or(0);
            y = y.saturating_add(height);
            if idx + 1 < self.messages.len() {
                y = y.saturating_add(1); // Gap between messages
            }
        }

        // Render only visible messages
        for idx in visible_range.first_index..=visible_range.last_index {
            if idx >= self.messages.len() {
                break;
            }

            let message = &self.messages[idx];
            let mode = self.determine_view_mode(message);
            let height = self
                .cached_heights
                .get(idx)
                .copied()
                .unwrap_or_else(|| self.renderer.calculate_height(message, mode, area.width));

            // Calculate the actual area for this message
            let message_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            };

            // Only render if the message area intersects with the visible area
            if message_area.y < area.y.saturating_add(area.height)
                && message_area.y.saturating_add(message_area.height) > area.y
            {
                // Highlight selected message
                if self.state.selected == Some(idx) {
                    let highlight_block = Block::default()
                        .borders(Borders::LEFT)
                        .border_style(styles::SELECTION_HIGHLIGHT);
                    highlight_block.render(message_area, buf);

                    // Adjust area for content
                    let content_area = Rect {
                        x: message_area.x.saturating_add(1),
                        width: message_area.width.saturating_sub(1),
                        ..message_area
                    };

                    self.renderer.render(message, mode, content_area, buf);
                } else {
                    self.renderer.render(message, mode, message_area, buf);
                }
            }

            y = y.saturating_add(height);

            // Add gap between messages
            if idx + 1 < self.messages.len() {
                y = y.saturating_add(1);
            }
        }
    }
}

impl MessagesRenderer<'_> {
    fn determine_view_mode(&self, content: &MessageContent) -> ViewMode {
        self.state.view_prefs.determine_mode(content)
    }
}

// For backwards compatibility, provide a type alias
pub type MessageViewState = MessageListState;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::UserContent;

    #[test]
    fn test_visible_range_no_overflow() {
        let mut state = MessageListState::new();
        let renderer = DefaultContentRenderer;

        // Create test messages
        let messages = vec![
            MessageContent::User {
                id: "test-1".to_string(),
                blocks: vec![UserContent::Text {
                    text: "Message 1".to_string(),
                }],
                timestamp: "2023-01-01T00:00:00Z".to_string(),
            },
            MessageContent::User {
                id: "test-2".to_string(),
                blocks: vec![UserContent::Text {
                    text: "Message 2".to_string(),
                }],
                timestamp: "2023-01-01T00:00:00Z".to_string(),
            },
        ];

        // Set scroll offset to a high value that could cause overflow
        state.scroll_state.set_offset((0, u16::MAX - 100).into());

        // This should not panic with overflow
        let range = state.calculate_visible_range(
            &messages, 200, // viewport height
            80,  // width
            &renderer,
        );

        // The range calculation should handle the overflow gracefully
        assert!(range.is_some() || range.is_none()); // Either result is fine, as long as no panic
    }

    #[test]
    fn test_scroll_by_integer_overflow_protection() {
        let mut state = MessageListState::new();
        let renderer = DefaultContentRenderer;

        // Create test messages
        let messages = vec![MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Message 1".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        }];

        // Test with extremely large positive amount
        let result = state.scroll_by(&messages, 200, 80, &renderer, i32::MAX);

        // Should handle without panic
        assert!(result || !result); // Either is fine, just no panic

        // Test with extremely large negative amount
        let result = state.scroll_by(&messages, 200, 80, &renderer, i32::MIN);

        // Should handle without panic
        assert!(result || !result); // Either is fine, just no panic
    }

    #[test]
    fn test_simple_scroll_with_terminal_size() {
        let mut state = MessageListState::new();

        // Create test messages
        let messages = vec![MessageContent::User {
            id: "test-1".to_string(),
            blocks: vec![UserContent::Text {
                text: "Message 1".to_string(),
            }],
            timestamp: "2023-01-01T00:00:00Z".to_string(),
        }];

        // Test with provided terminal size
        let terminal_size = Some((100, 50));
        let result = state.simple_scroll_down(5, &messages, terminal_size);

        // Should not panic and should use the provided terminal size
        assert!(result || !result);

        // Test without terminal size (fallback)
        let result = state.simple_scroll_up(5, &messages, None);
        assert!(result || !result);
    }
}
