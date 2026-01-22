//! State management for chat list scrolling and view modes

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
    pub offset: usize,
    /// Pending scroll target to resolve during measurement
    scroll_target: Option<ScrollTarget>,
    /// View preferences
    pub view_mode: ViewMode,
    /// Cached visible range for efficient rendering
    pub visible_range: Option<VisibleRange>,
    /// Total content height (cached during render)
    pub total_content_height: usize,
    /// Viewport height (cached during render)
    pub last_viewport_height: u16,
    /// Track if user has manually scrolled away from bottom
    pub user_scrolled: bool,
}

#[derive(Debug, Clone)]
pub struct VisibleRange {
    pub first_index: usize,
    pub last_index: usize,
    pub first_y: u16,
    pub last_y: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollTarget {
    Bottom,
    Item(usize),
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
            scroll_target: None,
            view_mode: ViewMode::Compact,
            visible_range: None,
            total_content_height: 0,
            last_viewport_height: 0,
            user_scrolled: false,
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        // Resolve during render when we know total height
        self.scroll_target = Some(ScrollTarget::Bottom);
        self.user_scrolled = false;
    }

    pub fn scroll_up(&mut self, amount: usize) -> bool {
        self.scroll_target = None;
        let previous = self.offset;
        self.offset = self.offset.saturating_sub(amount);
        if let Some(max_offset) = self.max_offset() {
            self.offset = self.offset.min(max_offset);
        }
        if self.offset != previous {
            self.user_scrolled = true;
            true
        } else {
            false
        }
    }

    pub fn scroll_down(&mut self, amount: usize) -> bool {
        self.scroll_target = None;
        let previous = self.offset;
        self.offset = self.offset.saturating_add(amount);
        if let Some(max_offset) = self.max_offset() {
            self.offset = self.offset.min(max_offset);
        }
        if self.offset != previous {
            self.user_scrolled = true;
            true
        } else {
            false
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.offset = 0;
        self.scroll_target = None;
        self.user_scrolled = true;
    }

    pub fn is_at_bottom(&self) -> bool {
        // Check if we're at the bottom based on actual content height
        if self.total_content_height == 0 || self.last_viewport_height == 0 {
            return true;
        }

        let max_offset = self
            .total_content_height
            .saturating_sub(self.last_viewport_height as usize);
        // We're at bottom if offset is at max or if user hasn't manually scrolled
        !self.user_scrolled || self.offset >= max_offset
    }

    /// Scroll to center a specific item in the viewport
    pub fn scroll_to_item(&mut self, index: usize) {
        self.scroll_target = Some(ScrollTarget::Item(index));
        self.user_scrolled = true;
    }

    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Compact => ViewMode::Detailed,
            ViewMode::Detailed => ViewMode::Compact,
        };
    }

    pub fn take_scroll_target(&mut self) -> Option<ScrollTarget> {
        self.scroll_target.take()
    }

    fn max_offset(&self) -> Option<usize> {
        if self.total_content_height == 0 || self.last_viewport_height == 0 {
            None
        } else {
            Some(
                self.total_content_height
                    .saturating_sub(self.last_viewport_height as usize),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_noop_at_bottom() {
        let mut state = ChatListState::new();
        state.total_content_height = 100;
        state.last_viewport_height = 10;
        state.offset = 90;
        state.user_scrolled = false;

        let moved_down = state.scroll_down(5);
        assert!(!moved_down, "Scrolling down at bottom should be a no-op");
        assert_eq!(state.offset, 90, "Offset should remain at max");
        assert!(!state.user_scrolled, "No-op scroll should not mark user_scrolled");

        let moved_up = state.scroll_up(1);
        assert!(moved_up, "Scrolling up should move");
        assert_eq!(state.offset, 89, "Offset should decrease");
        assert!(state.user_scrolled, "User scroll should be tracked when moving");
    }
}
