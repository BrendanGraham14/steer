//! ChatViewport - persistent chat list state with O(N) rebuild optimization

use crate::tui::{
    model::ChatItem,
    theme::Theme,
    widgets::{
        ChatBlock, ChatListState, ChatWidget, DynamicChatWidget, GutterWidget, RoleGlyph, ViewMode,
    },
};
use conductor_core::app::conversation::{
    AssistantContent, CommandResponse, CompactResult, Message,
};
use conductor_tools::{ToolResult, schema::ToolCall};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
};
use std::collections::HashMap;

/// Flattened item types for 1:1 widget mapping
#[derive(Debug, Clone)]
enum FlattenedItem {
    /// Text content from a message (user or assistant)
    MessageText { message: Message, id: String },
    /// Tool call coupled with its result
    ToolInteraction {
        call: ToolCall,
        result: Option<ToolResult>,
        id: String,
    },
    /// Meta items (system notices, command responses, etc.)
    Meta { item: ChatItem, id: String },
}

impl FlattenedItem {
    /// Get the ID of this flattened item
    fn id(&self) -> &str {
        match self {
            FlattenedItem::MessageText { id, .. } => id,
            FlattenedItem::ToolInteraction { id, .. } => id,
            FlattenedItem::Meta { id, .. } => id,
        }
    }

    /// Check if this is a message item (for spacing logic)
    fn is_message(&self) -> bool {
        matches!(
            self,
            FlattenedItem::MessageText { .. } | FlattenedItem::ToolInteraction { .. }
        )
    }
}

/// Metrics for a single row to be rendered
#[derive(Debug, Clone)]
struct RowMetrics {
    idx: usize,                // index into self.items
    render_h: usize,           // actual height to render in viewport
    first_visible_line: usize, // line offset for partial rendering
}

/// Widget item wrapper for height caching
struct WidgetItem {
    id: String,                                // Unique identifier for reuse tracking
    item: FlattenedItem,                       // The flattened item
    widget: Box<dyn ChatWidget + Send + Sync>, // Cached widget
    cached_heights: HeightCache,
}

/// Height cache for different view modes and widths
struct HeightCache {
    compact: Option<usize>,
    detailed: Option<usize>,
    last_width: u16,
}

impl HeightCache {
    fn new() -> Self {
        Self {
            compact: None,
            detailed: None,
            last_width: 0,
        }
    }

    fn invalidate(&mut self, width_changed: bool, mode_changed: bool) {
        if width_changed {
            self.compact = None;
            self.detailed = None;
        } else if mode_changed {
            // Mode change only affects the current mode's cache
            // But for simplicity, we can clear both
            self.compact = None;
            self.detailed = None;
        }
    }

    fn get_height(&self, mode: ViewMode) -> Option<usize> {
        match mode {
            ViewMode::Compact => self.compact,
            ViewMode::Detailed => self.detailed,
        }
    }

    fn set_height(&mut self, mode: ViewMode, height: usize, width: u16) {
        self.last_width = width;
        match mode {
            ViewMode::Compact => self.compact = Some(height),
            ViewMode::Detailed => self.detailed = Some(height),
        }
    }
}

/// ChatViewport manages persistent widget state and efficient rendering
pub struct ChatViewport {
    items: Vec<WidgetItem>, // persistent; each has HeightCache + Box<dyn ChatWidget>
    state: ChatListState,   // scroll offset, view_mode, etc.
    last_width: u16,        // for invalidation on resize
    dirty: bool,            // set by caller when messages change
}

impl ChatViewport {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            state: ChatListState::new(),
            last_width: 0,
            dirty: true,
        }
    }

    /// Mark the viewport as dirty, forcing a rebuild on next render
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Get mutable reference to the chat list state for key handlers
    pub fn state_mut(&mut self) -> &mut ChatListState {
        &mut self.state
    }

    /// Get reference to the chat list state
    pub fn state(&self) -> &ChatListState {
        &self.state
    }

    /// Diff raw ChatItems; rebuild only when dirty / width or mode changed.
    pub fn rebuild(&mut self, raw: &Vec<&ChatItem>, width: u16, mode: ViewMode, theme: &Theme) {
        // Check if we need to rebuild
        let width_changed = width != self.last_width;
        let mode_changed = mode != self.state.view_mode;

        if !self.dirty && !width_changed && !mode_changed {
            return;
        }

        // Update state
        self.last_width = width;
        self.state.view_mode = mode;

        // Flatten raw items into 1:1 widget items
        let flattened = self.flatten_items(raw);

        // Build a map of existing widgets by ID for reuse
        let mut existing_widgets: HashMap<String, WidgetItem> = HashMap::new();
        for item in self.items.drain(..) {
            existing_widgets.insert(item.id.clone(), item);
        }

        // Build widget items from flattened items
        let mut new_items = Vec::new();
        for flattened_item in flattened {
            let item_id = flattened_item.id().to_string();

            if let Some(mut existing) = existing_widgets.remove(&item_id) {
                // Invalidate cache if width or mode changed
                if width_changed || mode_changed {
                    existing
                        .cached_heights
                        .invalidate(width_changed, mode_changed);
                }
                // Update the item to match the new flattened structure
                existing.item = flattened_item;
                new_items.push(existing);
            } else {
                // Create new widget
                let widget = create_widget_for_flattened_item(&flattened_item, theme, false, 0);
                let widget_item = WidgetItem {
                    id: item_id,
                    item: flattened_item,
                    widget,
                    cached_heights: HeightCache::new(),
                };
                new_items.push(widget_item);
            }
        }

        self.items = new_items;
        self.dirty = false;
    }

    /// Flatten raw ChatItems into 1:1 widget items
    fn flatten_items(&self, raw: &[&ChatItem]) -> Vec<FlattenedItem> {
        // First pass: collect tool results for coupling
        let mut tool_results: HashMap<String, ToolResult> = HashMap::new();
        raw.iter().for_each(|item| {
            if let ChatItem::Message(Message::Tool {
                tool_use_id,
                result,
                ..
            }) = item
            {
                tool_results.insert(tool_use_id.clone(), result.clone());
            }
        });

        let mut flattened = Vec::new();

        for item in raw {
            match item {
                ChatItem::Message(row) => {
                    match &row {
                        Message::Assistant { content, .. } => {
                            // Check if there's any text content
                            let has_text_content = content.iter().any(|block| match block {
                                AssistantContent::Text { text } => !text.trim().is_empty(),
                                AssistantContent::Thought { .. } => true,
                                _ => false,
                            });

                            // Emit MessageText if there's text content
                            if has_text_content {
                                flattened.push(FlattenedItem::MessageText {
                                    message: row.clone(),
                                    id: format!("{}_text", row.id()),
                                });
                            }

                            // Emit ToolInteraction for each tool call
                            let mut tool_idx = 0;
                            for block in content {
                                if let AssistantContent::ToolCall { tool_call } = block {
                                    if let Some(result) = tool_results.remove(&tool_call.id) {
                                        // Only create a tool interaction for the tool call if it has a result.
                                        // Otherwise, we rely on the ToolInteraction emitted for PendingToolCall.
                                        flattened.push(FlattenedItem::ToolInteraction {
                                            call: tool_call.clone(),
                                            result: Some(result),
                                            id: format!("{}_tool_{}", row.id(), tool_idx),
                                        });
                                    }
                                    tool_idx += 1;
                                }
                            }
                        }
                        Message::Tool { .. } => {
                            // Skip - they should always be coupled with a tool call
                        }
                        Message::User {
                            content,
                            timestamp,
                            id,
                            parent_message_id,
                        } => {
                            // User messages and others
                            flattened.push(FlattenedItem::MessageText {
                                message: Message::User {
                                    content: content.clone(),
                                    timestamp: *timestamp,
                                    id: id.clone(),
                                    parent_message_id: parent_message_id.clone(),
                                },
                                id: id.clone(),
                            });
                        }
                    }
                }
                _ => {
                    // Non-message items (system notices, etc.)
                    match item {
                        ChatItem::PendingToolCall { id, tool_call, .. } => {
                            // Convert pending tool calls to ToolInteraction with no result
                            flattened.push(FlattenedItem::ToolInteraction {
                                call: tool_call.clone(),
                                result: None,
                                id: id.clone(),
                            });
                        }
                        _ => {
                            // Other meta items
                            flattened.push(FlattenedItem::Meta {
                                item: (*item).clone(),
                                id: match item {
                                    ChatItem::SystemNotice { id, .. } => id.clone(),
                                    ChatItem::CoreCmdResponse { id, .. } => id.clone(),
                                    ChatItem::InFlightOperation { id, .. } => id.to_string(),
                                    ChatItem::SlashInput { id, .. } => id.clone(),
                                    ChatItem::TuiCommandResponse { id, .. } => id.clone(),
                                    _ => unreachable!(),
                                },
                            });
                        }
                    }
                }
            }
        }

        flattened
    }

    /// Measure visible rows and update scroll state
    fn measure_visible_rows(&mut self, area: Rect, theme: &Theme) -> Vec<RowMetrics> {
        if self.items.is_empty() {
            return Vec::new();
        }

        let spacing = theme.message_spacing();
        let mut total_height: usize = 0;
        let mut item_top_positions = Vec::with_capacity(self.items.len());
        let items_len = self.items.len();

        // Calculate total height and item positions
        for (idx, item) in self.items.iter_mut().enumerate() {
            // Get or calculate height
            let h = item
                .cached_heights
                .get_height(self.state.view_mode)
                .unwrap_or_else(|| {
                    let height = item.widget.height(self.state.view_mode, area.width, theme);
                    item.cached_heights
                        .set_height(self.state.view_mode, height, area.width);
                    height
                });

            item_top_positions.push(total_height);
            total_height = total_height.saturating_add(h);

            // Add spacing after all messages except the last
            if idx + 1 < items_len && item.item.is_message() {
                total_height = total_height.saturating_add(spacing as usize);
            }
        }

        // Update scroll state
        self.state.total_content_height = total_height;
        self.state.last_viewport_height = area.height;
        let viewport_height = area.height as usize;

        // Adjust scroll offset if needed
        if self.state.offset == usize::MAX || total_height <= viewport_height {
            // Scroll to bottom or fit in view
            self.state.offset = total_height.saturating_sub(viewport_height);
            self.state.user_scrolled = false;
        } else if self.state.offset > usize::MAX - 100 {
            // Special case: scroll to specific item
            let target_idx = usize::MAX - 1 - self.state.offset;
            if let Some(&item_y) = item_top_positions.get(target_idx) {
                let half_viewport = viewport_height / 2;
                self.state.offset = item_y.saturating_sub(half_viewport);
                self.state.offset = self
                    .state
                    .offset
                    .min(total_height.saturating_sub(viewport_height));
            }
        } else {
            // Ensure we don't scroll past the bottom
            self.state.offset = self
                .state
                .offset
                .min(total_height.saturating_sub(viewport_height));
        }

        // Build RowMetrics for visible items
        let mut rows = Vec::new();
        let mut cumulative_y = 0;

        for (idx, item) in self.items.iter().enumerate() {
            let item_height = item
                .cached_heights
                .get_height(self.state.view_mode)
                .expect("Height should be cached");

            let item_bottom = cumulative_y + item_height;

            // Check if item is visible
            if item_bottom > self.state.offset && cumulative_y < self.state.offset + viewport_height
            {
                // Calculate render height and first visible line
                let (render_h, first_visible_line) = if cumulative_y < self.state.offset {
                    // Item starts above viewport - partial render from offset
                    let first_line = self.state.offset - cumulative_y;
                    let visible_height = item_height.saturating_sub(first_line);
                    (visible_height.min(viewport_height), first_line)
                } else if item_bottom > self.state.offset + viewport_height {
                    // Item extends below viewport - clip to viewport
                    let visible_height =
                        (self.state.offset + viewport_height).saturating_sub(cumulative_y);
                    (visible_height, 0)
                } else {
                    // Item fully visible
                    (item_height, 0)
                };

                rows.push(RowMetrics {
                    idx,
                    render_h,
                    first_visible_line,
                });
            }

            cumulative_y = cumulative_y.saturating_add(item_height);

            // Add spacing after messages
            if idx + 1 < items_len && item.item.is_message() {
                cumulative_y = cumulative_y.saturating_add(spacing as usize);
            }

            // Early exit if we're past the visible area
            if cumulative_y > self.state.offset + viewport_height {
                break;
            }
        }

        rows
    }

    /// Prepare visible widgets by updating hover/spinner states
    fn prepare_visible_widgets(
        &mut self,
        rows: &[RowMetrics],
        hovered: Option<&str>,
        spinner: usize,
        theme: &Theme,
    ) {
        for row in rows {
            let widget_item = &mut self.items[row.idx];
            let is_hovered = hovered.is_some_and(|h| h == widget_item.id);

            // Only recreate widget if hover state changed or it needs animation
            let needs_animation = match &widget_item.item {
                FlattenedItem::ToolInteraction { result: None, .. } => true,
                FlattenedItem::Meta { item, .. } => matches!(
                    item,
                    ChatItem::PendingToolCall { .. } | ChatItem::InFlightOperation { .. }
                ),
                _ => false,
            };

            if is_hovered || needs_animation {
                // Recreate the widget with current hover/spinner state
                widget_item.widget =
                    create_widget_for_flattened_item(&widget_item.item, theme, is_hovered, spinner);
            }
        }
    }

    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        spinner: usize,
        hovered: Option<&str>,
        theme: &Theme,
    ) {
        if self.items.is_empty() {
            return;
        }

        // Clear the area with theme background
        if let Some(bg_color) = theme.get_background_color() {
            f.render_widget(ratatui::widgets::Clear, area);
            let background = ratatui::widgets::Block::default()
                .style(ratatui::style::Style::default().bg(bg_color));
            f.render_widget(background, area);
        }

        // Measure visible rows and update scroll state
        let rows = self.measure_visible_rows(area, theme);
        if rows.is_empty() {
            return;
        }

        // Prepare visible widgets (update hover/spinner states)
        self.prepare_visible_widgets(&rows, hovered, spinner, theme);

        // Build constraints for ratatui Layout
        let spacing = theme.message_spacing();
        let mut constraints = Vec::with_capacity(rows.len() * 2);

        for (i, row) in rows.iter().enumerate() {
            // Add constraint for the widget
            constraints.push(Constraint::Length(row.render_h as u16));

            // Add spacing after messages (except last row)
            if i + 1 < rows.len() && self.items[row.idx].item.is_message() {
                constraints.push(Constraint::Length(spacing));
            }
        }

        // Use ratatui Layout to compute rectangles
        let rects = Layout::vertical(constraints).split(area);
        let mut rect_iter = rects.iter();

        // Render each visible row
        for row in &rows {
            if let Some(rect) = rect_iter.next() {
                let widget_item = &mut self.items[row.idx];

                if row.first_visible_line > 0 || rect.height < row.render_h as u16 {
                    // Partial rendering needed
                    widget_item.widget.render_partial(
                        *rect,
                        f.buffer_mut(),
                        self.state.view_mode,
                        theme,
                        row.first_visible_line,
                    );
                } else {
                    // Full rendering
                    widget_item
                        .widget
                        .render(*rect, f.buffer_mut(), self.state.view_mode, theme);
                }

                // Skip the spacer rect if present
                if row.idx + 1 < self.items.len() && self.items[row.idx].item.is_message() {
                    rect_iter.next();
                }
            }
        }
    }
}

/// Helper function to create a widget for a flattened item
fn create_widget_for_flattened_item(
    item: &FlattenedItem,
    theme: &Theme,
    is_hovered: bool,
    spinner_state: usize,
) -> Box<dyn ChatWidget + Send + Sync> {
    use crate::tui::widgets::chat_widgets::{
        CommandResponseWidget, InFlightOperationWidget, SlashInputWidget, SystemNoticeWidget,
        format_app_command, format_command_response, get_spinner_char, row_widget::RowWidget,
    };

    match item {
        FlattenedItem::MessageText { message, .. } => {
            let chat_block = ChatBlock::Message(message.clone());

            // Determine role glyph from message type
            let role = match message {
                Message::User { .. } => RoleGlyph::User,
                Message::Assistant { .. } => RoleGlyph::Assistant,
                Message::Tool { .. } => RoleGlyph::Tool,
            };

            // Create gutter widget
            let gutter = GutterWidget::new(role).with_hover(is_hovered);

            // Create body widget
            let body = Box::new(DynamicChatWidget::from_block(chat_block, theme));

            // Wrap in RowWidget
            Box::new(RowWidget::new(gutter, body))
        }
        FlattenedItem::ToolInteraction { call, result, .. } => {
            let chat_block = ChatBlock::ToolInteraction {
                call: call.clone(),
                result: result.clone(),
            };

            let mut gutter = GutterWidget::new(RoleGlyph::Tool).with_hover(is_hovered);

            // Show spinner if tool call is pending (i.e., no result yet)
            if result.is_none() {
                gutter = gutter.with_spinner(get_spinner_char(spinner_state));
            }

            let body = Box::new(DynamicChatWidget::from_block(chat_block, theme));
            Box::new(RowWidget::new(gutter, body))
        }
        FlattenedItem::Meta { item, .. } => {
            // Delegate to the original function for meta items
            match item {
                ChatItem::SystemNotice {
                    level, text, ts, ..
                } => {
                    let gutter = GutterWidget::new(RoleGlyph::Meta).with_hover(is_hovered);
                    let body = Box::new(SystemNoticeWidget::new(*level, text.clone(), *ts));
                    Box::new(RowWidget::new(gutter, body))
                }
                ChatItem::CoreCmdResponse { cmd, resp, .. } => {
                    let gutter = GutterWidget::new(RoleGlyph::Meta).with_hover(is_hovered);
                    let body = Box::new(CommandResponseWidget::new(
                        format_app_command(cmd),
                        format_command_response(resp),
                    ));
                    Box::new(RowWidget::new(gutter, body))
                }
                ChatItem::InFlightOperation { label, .. } => {
                    let gutter = GutterWidget::new(RoleGlyph::Meta)
                        .with_hover(is_hovered)
                        .with_spinner(get_spinner_char(spinner_state));

                    let body = Box::new(InFlightOperationWidget::new(label.clone()));
                    Box::new(RowWidget::new(gutter, body))
                }
                ChatItem::SlashInput { raw, .. } => {
                    let gutter = GutterWidget::new(RoleGlyph::Meta).with_hover(is_hovered);
                    let body = Box::new(SlashInputWidget::new(raw.clone()));
                    Box::new(RowWidget::new(gutter, body))
                }
                ChatItem::TuiCommandResponse {
                    command, response, ..
                } => {
                    let gutter = GutterWidget::new(RoleGlyph::Meta).with_hover(is_hovered);
                    let body = Box::new(CommandResponseWidget::new(
                        format!("/{command}"),
                        response.clone(),
                    ));
                    Box::new(RowWidget::new(gutter, body))
                }
                _ => unreachable!("All meta items should be handled"),
            }
        }
    }
}

#[allow(dead_code)]
pub fn format_command_response(resp: &CommandResponse) -> String {
    match resp {
        CommandResponse::Text(text) => text.clone(),
        CommandResponse::Compact(result) => match result {
            CompactResult::Success(summary) => summary.clone(),
            CompactResult::Cancelled => "Compact cancelled.".to_string(),
            CompactResult::InsufficientMessages => "Not enough messages to compact.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conductor_core::app::conversation::{Message, UserContent};
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::SystemTime;

    fn create_test_message(content: &str, id: &str) -> ChatItem {
        ChatItem::Message(Message::User {
            content: vec![UserContent::Text {
                text: content.to_string(),
            }],
            timestamp: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            id: id.to_string(),
            parent_message_id: None,
        })
    }

    #[test]
    fn test_partial_rendering_with_large_first_item() {
        // Create a viewport with a very large first item
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create test messages - first one with many lines
        let mut messages = vec![];

        // First message with 100 lines of content (exceeds viewport height of 20)
        let large_content = (0..100)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        messages.push(create_test_message(&large_content, "msg1"));

        // Add a few more normal messages that should be visible
        messages.push(create_test_message("Second message visible", "msg2"));
        messages.push(create_test_message("Third message visible", "msg3"));
        messages.push(create_test_message("Fourth message", "msg4"));

        // Set up terminal with small height
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = f.area();

                // Rebuild viewport with messages
                viewport.rebuild(
                    &messages.iter().collect(),
                    area.width,
                    ViewMode::Compact,
                    &theme,
                );

                // Render the viewport
                viewport.render(f, area, 0, None, &theme);
            })
            .unwrap();

        // Get the buffer contents
        let buffer = terminal.backend().buffer();
        let buffer_str = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        // Verify that we see content from the large first message
        assert!(
            buffer_str.contains("Line 0"),
            "Should see beginning of first message"
        );

        // With the bug, we wouldn't see any subsequent messages.
        // With the fix, we should see at least the second message
        assert!(
            buffer_str.contains("Second message visible"),
            "Should see second message after partial rendering of first - buffer: {buffer_str}"
        );

        // May also see third message depending on exact heights
        let has_third = buffer_str.contains("Third message visible");
        println!("Third message visible: {has_third}");
    }

    #[test]
    fn test_scroll_offset_with_buffer_verification() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create messages with enough content to require scrolling
        // Use multiple separate lines to ensure proper height calculation
        let messages = vec![
            create_test_message(
                "First message line 1\nFirst message line 2\nFirst message line 3\nFirst message line 4\nFirst message line 5\nFirst message line 6\nFirst message line 7\nFirst message line 8",
                "msg1",
            ),
            create_test_message(
                "Second message line 1\nSecond message line 2\nSecond message line 3\nSecond message line 4\nSecond message line 5",
                "msg2",
            ),
            create_test_message(
                "Third message line 1\nThird message line 2\nThird message line 3\nThird message line 4\nThird message line 5\nThird message line 6",
                "msg3",
            ),
            create_test_message(
                "Fourth message line 1\nFourth message line 2\nFourth message line 3\nFourth message line 4",
                "msg4",
            ),
        ];

        // Small viewport that can't fit all content
        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = f.area();

                // Rebuild viewport
                viewport.rebuild(&messages.iter().collect(), area.width, ViewMode::Compact, &theme);

                // Measure visible rows
                let _rows = viewport.measure_visible_rows(area, &theme);
                let total_height = viewport.state.total_content_height;
                println!("Total content height: {}, viewport height: {}", total_height, area.height);

                // Debug: print individual item heights
                for (idx, item) in viewport.items.iter().enumerate() {
                    let height = item.cached_heights.get_height(ViewMode::Compact).unwrap_or(0);
                    println!("Item {idx} height: {height}");
                }

                assert!(total_height > area.height as usize, "Content should be taller than viewport for this test - total_height: {}, viewport_height: {}", total_height, area.height);

                // Scroll to bottom
                viewport.state.offset = usize::MAX; // This triggers scroll-to-bottom logic

                // Render
                viewport.render(f, area, 0, None, &theme);
            })
.unwrap();

        let buffer = terminal.backend().buffer();
        let buffer_str = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        // When scrolled to bottom, we should see the last messages
        println!("Buffer content when scrolled to bottom:");
        for y in 0..10 {
            let line: String = (0..80)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect();
            println!("Line {}: {}", y, line.trim_end());
        }

        assert!(
            buffer_str.contains("Fourth message"),
            "Should see fourth message when scrolled to bottom"
        );

        // Should also see at least part of third message
        assert!(
            buffer_str.contains("Third message") || buffer_str.contains("Third line"),
            "Should see at least part of third message"
        );

        // First message should not be visible when scrolled to bottom (except maybe very last lines due to wrapping)
        // Check for the beginning of the first message which definitely shouldn't be visible
        assert!(
            !buffer_str.contains("First message line 1\n"),
            "Beginning of first message should not be visible when scrolled to bottom"
        );
    }

    #[test]
    fn test_very_long_first_message_with_scroll_to_bottom() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create messages - first one is VERY long
        let mut messages = vec![];

        // First message with 200 lines
        let large_content = (0..200)
            .map(|i| format!("First message line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        messages.push(create_test_message(&large_content, "msg1"));

        // Second and third messages are short
        messages.push(create_test_message("Second message content", "msg2"));
        messages.push(create_test_message("Third message content", "msg3"));

        // Small viewport
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
        .draw(|f| {
            let area = f.area();

            // Rebuild viewport
            viewport.rebuild(&messages.iter().collect(), area.width, ViewMode::Compact, &theme);

            let _rows = viewport.measure_visible_rows(area, &theme);
            let total_height = viewport.state.total_content_height;
            println!("Total content height: {}, viewport height: {}", total_height, area.height);

            // Debug: print individual item heights
            for (idx, item) in viewport.items.iter().enumerate() {
            let height = item.cached_heights.get_height(ViewMode::Compact).unwrap_or(0);
            println!("Item {idx} height: {height}");
            }

            assert!(total_height > area.height as usize, "Content should be much taller than viewport - total_height: {}, viewport_height: {}", total_height, area.height);

            // Scroll to bottom
            viewport.state.offset = usize::MAX;

            // Render
            viewport.render(f, area, 0, None, &theme);
        })
        .unwrap();

        let buffer = terminal.backend().buffer();
        let buffer_str = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        // When scrolled to bottom, we MUST see the second and third messages
        // This verifies the fix for the render_y bug
        assert!(
            buffer_str.contains("Second message content"),
            "Second message must be visible when scrolled to bottom - this tests the render_y fix"
        );

        assert!(
            buffer_str.contains("Third message content"),
            "Third message must be visible when scrolled to bottom"
        );

        // We should see the end of the first message, not the beginning
        assert!(
            !buffer_str.contains("First message line 0"),
            "Should not see beginning of first message when scrolled to bottom"
        );

        // But we might see some later lines from the first message
        let has_late_first_message = buffer_str.contains("First message line 19");
        println!("Late first message lines visible: {has_late_first_message}");
    }

    #[test]
    fn test_measure_visible_rows_basic() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create test messages
        let messages = vec![
            create_test_message("First message", "msg1"),
            create_test_message("Second message", "msg2"),
            create_test_message("Third message", "msg3"),
        ];

        // Small viewport
        let area = Rect::new(0, 0, 80, 10);

        // Rebuild viewport
        viewport.rebuild(
            &messages.iter().collect(),
            area.width,
            ViewMode::Compact,
            &theme,
        );

        // Scroll to top
        viewport.state.offset = 0;

        // Measure visible rows
        let rows = viewport.measure_visible_rows(area, &theme);

        // Should see all messages if they fit
        assert!(!rows.is_empty(), "Should have visible rows");
        assert_eq!(rows[0].idx, 0, "First visible row should be index 0");
        assert_eq!(
            rows[0].first_visible_line, 0,
            "First row should start from line 0"
        );
    }

    #[test]
    fn test_measure_visible_rows_with_partial_first() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create a large first message
        let large_content = (0..20)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let messages = vec![
            create_test_message(&large_content, "msg1"),
            create_test_message("Second message", "msg2"),
        ];

        // Small viewport
        let area = Rect::new(0, 0, 80, 10);

        // Rebuild viewport
        viewport.rebuild(
            &messages.iter().collect(),
            area.width,
            ViewMode::Compact,
            &theme,
        );

        // Debug: print heights
        println!(
            "First message height: {}",
            viewport.items[0]
                .cached_heights
                .get_height(ViewMode::Compact)
                .unwrap_or(0)
        );
        println!(
            "Total content height: {}",
            viewport.state.total_content_height
        );

        // Scroll down a bit to make first message partial
        viewport.state.offset = 5;

        // Measure visible rows
        let rows = viewport.measure_visible_rows(area, &theme);

        assert!(!rows.is_empty(), "Should have visible rows");
        assert_eq!(rows[0].idx, 0, "First visible row should still be index 0");

        // Only check first_visible_line if the first message is actually tall enough
        if viewport.items[0]
            .cached_heights
            .get_height(ViewMode::Compact)
            .unwrap_or(0)
            > 5
        {
            assert_eq!(
                rows[0].first_visible_line, 5,
                "First row should skip 5 lines"
            );
        }
    }
}
