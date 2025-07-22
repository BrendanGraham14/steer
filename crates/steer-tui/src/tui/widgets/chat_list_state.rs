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
        self.offset = usize::MAX;
        self.user_scrolled = false;
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.offset = self.offset.saturating_sub(amount);
        self.user_scrolled = true;
    }

    pub fn scroll_down(&mut self, amount: usize) {
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
            .saturating_sub(self.last_viewport_height as usize);
        // We're at bottom if offset is at max or if user hasn't manually scrolled
        !self.user_scrolled || self.offset >= max_offset
    }

    /// Scroll to center a specific item in the viewport
    pub fn scroll_to_item(&mut self, index: usize) {
        // Special encoding: use high values to indicate specific item scrolling
        // We use u16::MAX - 1 - index to encode the target item
        if index < (usize::MAX - 100) {
            self.offset = usize::MAX - 1 - index;
            self.user_scrolled = true;
        }
    }

    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Compact => ViewMode::Detailed,
            ViewMode::Detailed => ViewMode::Compact,
        };
    }
}
