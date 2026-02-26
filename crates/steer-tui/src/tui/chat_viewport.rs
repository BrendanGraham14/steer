//! ChatViewport - persistent chat list state with O(N) rebuild optimization

use crate::tui::core_commands::{CommandResponse, CompactResult};
use crate::tui::{
    model::{ChatItem, ChatItemData},
    state::chat_store::ChatStore,
    theme::{Component, Theme},
    widgets::{
        ChatBlock, ChatListState, ChatRenderable, DynamicChatWidget, ScrollTarget, ViewMode,
        VisibleRange,
    },
};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use steer_grpc::client_api::{AssistantContent, Message, MessageData, UserContent};
use steer_tools::{ToolResult, schema::ToolCall};

/// Flattened item types for 1:1 widget mapping
#[derive(Debug, Clone)]
enum FlattenedItem {
    /// Text content from a message (user or assistant)
    MessageText {
        message: Message,
        id: String,
        is_edited: bool,
        is_editing: bool,
        is_compaction_summary: bool,
    },
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

    fn content_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        match self {
            FlattenedItem::MessageText {
                message,
                is_edited,
                is_editing,
                is_compaction_summary,
                ..
            } => {
                hash_message_content(message, &mut hasher);
                is_edited.hash(&mut hasher);
                is_editing.hash(&mut hasher);
                is_compaction_summary.hash(&mut hasher);
            }
            FlattenedItem::ToolInteraction { call, result, .. } => {
                call.id.hash(&mut hasher);
                call.name.hash(&mut hasher);
                call.parameters.to_string().hash(&mut hasher);
                if let Some(result) = result {
                    use std::fmt::Write as _;
                    let mut s = String::new();
                    let _ = write!(&mut s, "{result:?}");
                    s.hash(&mut hasher);
                }
            }
            FlattenedItem::Meta { item, .. } => {
                item.id().hash(&mut hasher);
                use std::fmt::Write as _;
                let mut s = String::new();
                let _ = write!(&mut s, "{:?}", item.data);
                s.hash(&mut hasher);
            }
        }
        hasher.finish()
    }
}

#[derive(Debug, Clone, Copy)]
enum RowKind {
    Item { idx: usize },
    Gap,
}

/// Metrics for a single row to be rendered
#[derive(Debug, Clone)]
struct RowMetrics {
    kind: RowKind,
    render_h: usize,           // actual height to render in viewport
    first_visible_line: usize, // line offset for partial rendering
}

#[derive(Debug, Clone, Copy)]
struct Segment {
    kind: RowKind,
    start_y: usize,
    height: usize,
}

/// Widget item wrapper for height caching
struct WidgetItem {
    id: String,                                    // Unique identifier for reuse tracking
    item: FlattenedItem,                           // The flattened item
    widget: Box<dyn ChatRenderable + Send + Sync>, // Cached widget
    cached_heights: HeightCache,
    content_hash: u64,
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
    segments: Vec<Segment>, // persistent segment index for viewport math
    item_start_y: Vec<usize>, // top Y per item index
    total_content_height: usize, // cached content height from segment index
    state: ChatListState,   // scroll offset, view_mode, etc.
    last_width: u16,        // for invalidation on resize
    last_spacing: u16,      // for invalidation when theme spacing changes
    last_rebuild_mode: ViewMode, // mode used for the last segment rebuild
    dirty: bool,            // set by caller when messages change
}

impl Default for ChatViewport {
    fn default() -> Self {
        Self::new()
    }
}

struct RebuildContext<'a> {
    theme: &'a Theme,
    chat_store: &'a ChatStore,
    editing_message_id: Option<&'a str>,
    spacing: u16,
}

impl ChatViewport {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            segments: Vec::new(),
            item_start_y: Vec::new(),
            total_content_height: 0,
            state: ChatListState::new(),
            last_width: 0,
            last_spacing: 0,
            last_rebuild_mode: ViewMode::Compact,
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
    pub fn rebuild(
        &mut self,
        raw: &[&ChatItem],
        width: u16,
        mode: ViewMode,
        theme: &Theme,
        chat_store: &ChatStore,
        editing_message_id: Option<&str>,
    ) {
        self.rebuild_with_context(
            Some(raw),
            width,
            mode,
            RebuildContext {
                theme,
                chat_store,
                editing_message_id,
                spacing: theme.message_spacing(),
            },
        );
    }

    /// Rebuild from ChatStore directly to avoid allocating a temporary item vector per frame.
    pub fn rebuild_from_store(
        &mut self,
        width: u16,
        mode: ViewMode,
        theme: &Theme,
        chat_store: &ChatStore,
        editing_message_id: Option<&str>,
    ) {
        self.rebuild_with_context(
            None,
            width,
            mode,
            RebuildContext {
                theme,
                chat_store,
                editing_message_id,
                spacing: theme.message_spacing(),
            },
        );
    }

    #[cfg(test)]
    fn rebuild_for_test(
        &mut self,
        raw: &[&ChatItem],
        width: u16,
        mode: ViewMode,
        theme: &Theme,
        chat_store: &ChatStore,
        editing_message_id: Option<&str>,
        spacing: u16,
    ) {
        self.rebuild_with_context(
            Some(raw),
            width,
            mode,
            RebuildContext {
                theme,
                chat_store,
                editing_message_id,
                spacing,
            },
        );
    }

    fn rebuild_with_context(
        &mut self,
        raw: Option<&[&ChatItem]>,
        width: u16,
        mode: ViewMode,
        context: RebuildContext<'_>,
    ) {
        let RebuildContext {
            theme,
            chat_store,
            editing_message_id,
            spacing,
        } = context;

        // Check if we need to rebuild
        let width_changed = width != self.last_width;
        let mode_changed = mode != self.last_rebuild_mode;
        let spacing_changed = spacing != self.last_spacing;

        if !self.dirty && !width_changed && !mode_changed && !spacing_changed {
            return;
        }

        // Update state
        self.last_width = width;
        self.last_spacing = spacing;
        self.last_rebuild_mode = mode;
        self.state.view_mode = mode;

        if !self.dirty {
            if width_changed || mode_changed {
                for item in &mut self.items {
                    item.cached_heights.invalidate(width_changed, mode_changed);
                }
            }

            self.rebuild_segment_index(width, mode, spacing as usize, theme);
            self.state.total_content_height = self.total_content_height;
            self.state.visible_range = None;
            return;
        }

        let raw_items: Cow<'_, [&ChatItem]> = match raw {
            Some(raw) => Cow::Borrowed(raw),
            None => Cow::Owned(chat_store.iter_items().collect()),
        };

        let edited_message_ids = build_edited_message_ids(chat_store);

        // Flatten raw items into 1:1 widget items.
        let flattened = if let Some(active_id) = &chat_store.active_message_id() {
            // Build lineage set by following parent_message_id chain.
            let lineage = build_lineage_set(active_id, chat_store);

            // Include pre-compaction history by stitching in ancestors of compaction heads.
            let visible_messages = build_visible_message_set(&lineage, chat_store);

            // Filter items to only show those in the visible message set or attached to it.
            let filtered_items = raw_items
                .iter()
                .filter(|item| is_visible(item, &visible_messages, chat_store))
                .copied()
                .collect::<Vec<_>>();

            Self::flatten_items(
                &filtered_items,
                &edited_message_ids,
                editing_message_id,
                chat_store,
            )
        } else {
            // No active branch - show everything.
            Self::flatten_items(
                &raw_items,
                &edited_message_ids,
                editing_message_id,
                chat_store,
            )
        };

        // Build a map of existing widgets by ID for reuse
        let mut existing_widgets: HashMap<String, WidgetItem> = HashMap::new();
        for item in self.items.drain(..) {
            existing_widgets.insert(item.id.clone(), item);
        }

        // Build widget items from flattened items
        let mut new_items = Vec::new();
        for flattened_item in flattened {
            let item_id = flattened_item.id().to_string();
            let content_hash = flattened_item.content_hash();

            if let Some(mut existing) = existing_widgets.remove(&item_id) {
                if existing.content_hash != content_hash {
                    existing.widget =
                        create_widget_for_flattened_item(&flattened_item, theme, false, 0);
                    existing.cached_heights.invalidate(true, true);
                } else if width_changed || mode_changed {
                    existing
                        .cached_heights
                        .invalidate(width_changed, mode_changed);
                }
                existing.item = flattened_item;
                existing.content_hash = content_hash;
                new_items.push(existing);
            } else {
                // Create new widget
                let widget = create_widget_for_flattened_item(&flattened_item, theme, false, 0);
                let widget_item = WidgetItem {
                    id: item_id,
                    item: flattened_item,
                    widget,
                    cached_heights: HeightCache::new(),
                    content_hash,
                };
                new_items.push(widget_item);
            }
        }

        self.items = new_items;
        self.rebuild_segment_index(width, mode, spacing as usize, theme);
        self.state.total_content_height = self.total_content_height;
        self.state.visible_range = None;
        self.dirty = false;
    }

    fn rebuild_segment_index(&mut self, width: u16, mode: ViewMode, spacing: usize, theme: &Theme) {
        self.segments.clear();
        self.item_start_y.clear();
        self.total_content_height = 0;

        self.item_start_y.reserve(self.items.len());
        self.segments.reserve(self.items.len().saturating_mul(2));

        let mut cursor = 0usize;
        let items_len = self.items.len();

        for (idx, item) in self.items.iter_mut().enumerate() {
            let height = item.cached_heights.get_height(mode).unwrap_or_else(|| {
                let measured = item.widget.line_count(width, mode, theme);
                item.cached_heights.set_height(mode, measured, width);
                measured
            });

            self.item_start_y.push(cursor);
            self.segments.push(Segment {
                kind: RowKind::Item { idx },
                start_y: cursor,
                height,
            });
            cursor = cursor.saturating_add(height);

            if idx + 1 < items_len && item.item.is_message() && spacing > 0 {
                self.segments.push(Segment {
                    kind: RowKind::Gap,
                    start_y: cursor,
                    height: spacing,
                });
                cursor = cursor.saturating_add(spacing);
            }
        }

        self.total_content_height = cursor;
    }

    fn first_visible_segment_index(&self, offset: usize) -> usize {
        self.segments
            .partition_point(|segment| segment.start_y.saturating_add(segment.height) <= offset)
    }

    /// Flatten raw ChatItems into 1:1 widget items
    fn flatten_items(
        raw: &[&ChatItem],
        edited_message_ids: &HashSet<String>,
        editing_message_id: Option<&str>,
        chat_store: &ChatStore,
    ) -> Vec<FlattenedItem> {
        // First pass: collect tool results for coupling
        let mut tool_results: HashMap<String, ToolResult> = HashMap::new();
        for item in raw {
            if let ChatItemData::Message(message) = &item.data
                && let MessageData::Tool {
                    tool_use_id,
                    result,
                    ..
                } = &message.data
            {
                tool_results.insert(tool_use_id.clone(), result.clone());
            }
        }

        let mut flattened = Vec::new();

        for item in raw {
            match &item.data {
                ChatItemData::Message(row) => {
                    match &row.data {
                        MessageData::Assistant { content, .. } => {
                            // Check if there's any text content
                            let has_text_content = content.iter().any(|block| match block {
                                AssistantContent::Text { text } => !text.trim().is_empty(),
                                AssistantContent::Image { .. } => true,
                                AssistantContent::Thought { .. } => true,
                                AssistantContent::ToolCall { .. } => false,
                            });

                            // Emit MessageText if there's text content
                            if has_text_content {
                                flattened.push(FlattenedItem::MessageText {
                                    message: row.clone(),
                                    id: format!("{}_text", row.id()),
                                    is_edited: false,
                                    is_editing: false,
                                    is_compaction_summary: chat_store
                                        .is_compaction_summary(row.id()),
                                });
                            }

                            // Emit ToolInteraction for each tool call
                            let mut tool_idx = 0;
                            for block in content {
                                if let AssistantContent::ToolCall { tool_call, .. } = block {
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
                        MessageData::Tool { .. } => {
                            // Skip - they should always be coupled with a tool call
                        }
                        MessageData::User { .. } => {
                            let is_edited = edited_message_ids.contains(row.id());
                            let is_editing =
                                editing_message_id.is_some_and(|editing_id| editing_id == row.id());
                            // User messages and others
                            flattened.push(FlattenedItem::MessageText {
                                message: row.clone(),
                                id: row.id().to_string(),
                                is_edited,
                                is_editing,
                                is_compaction_summary: false,
                            });
                        }
                    }
                }
                _ => {
                    // Non-message items (system notices, etc.)
                    match &item.data {
                        ChatItemData::PendingToolCall { id, tool_call, .. } => {
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
                                id: match &item.data {
                                    ChatItemData::SystemNotice { id, .. } => id.clone(),
                                    ChatItemData::CoreCmdResponse { id, .. } => id.clone(),
                                    ChatItemData::InFlightOperation { id, .. } => id.clone(),
                                    ChatItemData::SlashInput { id, .. } => id.clone(),
                                    ChatItemData::TuiCommandResponse { id, .. } => id.clone(),
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
    fn measure_visible_rows(&mut self, area: Rect) -> Vec<RowMetrics> {
        if self.items.is_empty() || self.segments.is_empty() {
            self.state.visible_range = None;
            self.state.total_content_height = self.total_content_height;
            self.state.last_viewport_height = area.height;
            return Vec::new();
        }

        self.state.total_content_height = self.total_content_height;
        self.state.last_viewport_height = area.height;

        let viewport_height = area.height as usize;
        let max_offset = self.total_content_height.saturating_sub(viewport_height);

        if let Some(target) = self.state.take_scroll_target() {
            match target {
                ScrollTarget::Bottom => {
                    self.state.offset = max_offset;
                    self.state.user_scrolled = false;
                }
                ScrollTarget::Item(target_idx) => {
                    if let Some(&item_y) = self.item_start_y.get(target_idx) {
                        let half_viewport = viewport_height / 2;
                        self.state.offset = item_y.saturating_sub(half_viewport);
                    }
                }
            }
        }

        self.state.offset = self.state.offset.min(max_offset);

        let viewport_bottom = self.state.offset.saturating_add(viewport_height);
        if viewport_height == 0 || self.state.offset >= viewport_bottom {
            self.state.visible_range = None;
            return Vec::new();
        }

        let first_segment_idx = self.first_visible_segment_index(self.state.offset);
        if first_segment_idx >= self.segments.len() {
            self.state.visible_range = None;
            return Vec::new();
        }

        let mut rows = Vec::new();
        let mut first_item_index: Option<usize> = None;
        let mut last_item_index: Option<usize> = None;
        let mut first_item_y: Option<u16> = None;
        let mut last_item_y: Option<u16> = None;

        let clamp_to_u16 = |value: usize| value.min(u16::MAX as usize) as u16;

        for segment in self.segments[first_segment_idx..].iter().copied() {
            if segment.start_y >= viewport_bottom {
                break;
            }

            let segment_bottom = segment.start_y.saturating_add(segment.height);
            if segment_bottom <= self.state.offset {
                continue;
            }

            let first_visible_line = self.state.offset.saturating_sub(segment.start_y);
            let render_end = segment_bottom.min(viewport_bottom);
            let render_h = render_end.saturating_sub(segment.start_y + first_visible_line);
            if render_h == 0 {
                continue;
            }

            if let RowKind::Item { idx } = segment.kind {
                if first_item_index.is_none() {
                    first_item_index = Some(idx);
                    first_item_y = Some(clamp_to_u16(
                        segment.start_y.saturating_sub(self.state.offset),
                    ));
                }
                last_item_index = Some(idx);
                let last_y = segment
                    .start_y
                    .saturating_add(render_h)
                    .saturating_sub(1)
                    .saturating_sub(self.state.offset);
                last_item_y = Some(clamp_to_u16(last_y));
            }

            rows.push(RowMetrics {
                kind: segment.kind,
                render_h,
                first_visible_line,
            });
        }

        self.state.visible_range =
            match (first_item_index, last_item_index, first_item_y, last_item_y) {
                (Some(first_index), Some(last_index), Some(first_y), Some(last_y)) => {
                    Some(VisibleRange {
                        first_index,
                        last_index,
                        first_y,
                        last_y,
                    })
                }
                _ => None,
            };

        rows
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, theme: &Theme) {
        if self.items.is_empty() {
            return;
        }

        if let Some(bg_color) = theme.get_background_color() {
            f.render_widget(ratatui::widgets::Clear, area);
            let background = ratatui::widgets::Block::default()
                .style(ratatui::style::Style::default().bg(bg_color));
            f.render_widget(background, area);
        }

        // Measure visible rows and update scroll state
        let rows = self.measure_visible_rows(area);
        if rows.is_empty() {
            return;
        }

        let mut y = area.y;
        let bottom = area.y.saturating_add(area.height);

        // Render each visible row directly with a running y cursor.
        for row in &rows {
            if y >= bottom {
                break;
            }

            let remaining = bottom.saturating_sub(y);
            let row_height = (row.render_h as u16).min(remaining);
            if row_height == 0 {
                continue;
            }

            let rect = Rect {
                x: area.x,
                y,
                width: area.width,
                height: row_height,
            };

            match row.kind {
                RowKind::Item { idx } => {
                    let widget_item = &mut self.items[idx];

                    let start = row.first_visible_line;
                    let end = start.saturating_add(rect.height as usize);
                    let lines = widget_item.widget.line_slice(
                        rect.width,
                        self.state.view_mode,
                        theme,
                        start,
                        end,
                    );
                    let buf = f.buffer_mut();
                    for (row_idx, line) in lines.iter().enumerate() {
                        let line_y = rect.y + row_idx as u16;
                        buf.set_line(rect.x, line_y, line, rect.width);
                    }
                }
                RowKind::Gap => {
                    if rect.width > 0 && rect.height > 0 {
                        let gap_style = theme.style(Component::ChatListBackground);
                        let gap_line =
                            Line::from(Span::styled(" ".repeat(rect.width as usize), gap_style));
                        let buf = f.buffer_mut();
                        for dy in 0..rect.height {
                            buf.set_line(rect.x, rect.y + dy, &gap_line, rect.width);
                        }
                    }
                }
            }

            y = y.saturating_add(row_height);
        }
    }
}

fn hash_message_content(message: &Message, hasher: &mut impl Hasher) {
    match &message.data {
        MessageData::User { content } => {
            for c in content {
                match c {
                    UserContent::Text { text } => text.hash(hasher),
                    UserContent::Image { image } => {
                        image.mime_type.hash(hasher);
                    }
                    UserContent::CommandExecution {
                        command,
                        stdout,
                        stderr,
                        exit_code,
                    } => {
                        command.hash(hasher);
                        stdout.hash(hasher);
                        stderr.hash(hasher);
                        exit_code.hash(hasher);
                    }
                }
            }
        }
        MessageData::Assistant { content } => {
            for b in content {
                match b {
                    AssistantContent::Text { text } => text.hash(hasher),
                    AssistantContent::Image { image } => {
                        image.mime_type.hash(hasher);
                    }
                    AssistantContent::ToolCall { tool_call, .. } => {
                        tool_call.id.hash(hasher);
                        tool_call.name.hash(hasher);
                        tool_call.parameters.to_string().hash(hasher);
                    }
                    AssistantContent::Thought { thought } => {
                        thought.display_text().hash(hasher);
                    }
                }
            }
        }
        MessageData::Tool {
            tool_use_id,
            result,
        } => {
            tool_use_id.hash(hasher);
            use std::fmt::Write as _;
            let mut s = String::new();
            let _ = write!(&mut s, "{result:?}");
            s.hash(hasher);
        }
    }
}

fn create_widget_for_flattened_item(
    item: &FlattenedItem,
    theme: &Theme,
    _is_hovered: bool,
    _spinner_state: usize,
) -> Box<dyn ChatRenderable + Send + Sync> {
    use crate::tui::widgets::chat_widgets::{
        CommandResponseWidget, InFlightOperationWidget, SlashInputWidget, SystemNoticeWidget,
        format_app_command, row_widget::RowWidget,
    };

    match item {
        FlattenedItem::MessageText {
            message,
            is_edited,
            is_editing,
            is_compaction_summary,
            ..
        } => {
            let body = Box::new(
                crate::tui::widgets::chat_widgets::message_widget::MessageWidget::new(
                    message.clone(),
                )
                .with_edited_indicator(*is_edited),
            );

            match &message.data {
                MessageData::User { .. } => {
                    let (user_message_style, accent_style) = if *is_editing {
                        (
                            theme.style(Component::UserMessageEdit),
                            theme.style(Component::UserMessageEditAccent),
                        )
                    } else {
                        (
                            theme.style(Component::UserMessage),
                            theme.style(Component::UserMessageAccent),
                        )
                    };
                    Box::new(
                        RowWidget::new(body)
                            .with_accent(accent_style)
                            .with_row_background(user_message_style)
                            .with_padding_lines(),
                    )
                }
                MessageData::Assistant { .. } => {
                    let mut row = RowWidget::new(body);
                    if *is_compaction_summary {
                        let sep_style = theme.style(Component::NoticeInfo);
                        row = row.with_separator_above(sep_style);
                    }
                    Box::new(row)
                }
                MessageData::Tool { .. } => Box::new(RowWidget::new(body)),
            }
        }
        FlattenedItem::ToolInteraction { call, result, .. } => {
            let chat_block = ChatBlock::ToolInteraction {
                call: call.clone(),
                result: result.clone(),
            };
            let body = Box::new(DynamicChatWidget::from_block(chat_block, theme));
            Box::new(RowWidget::new(body))
        }
        FlattenedItem::Meta { item, .. } => {
            let accent_style = theme.style(Component::SystemMessageAccent);

            match &item.data {
                ChatItemData::SystemNotice {
                    level, text, ts, ..
                } => {
                    let body = Box::new(SystemNoticeWidget::new(*level, text.clone(), *ts));
                    Box::new(RowWidget::new(body))
                }
                ChatItemData::CoreCmdResponse {
                    command: cmd,
                    response,
                    ..
                } => {
                    let body = Box::new(CommandResponseWidget::new(
                        format_app_command(cmd),
                        response.clone().into(),
                    ));
                    Box::new(RowWidget::new(body).with_accent(accent_style))
                }
                ChatItemData::InFlightOperation { label, .. } => {
                    let body = Box::new(InFlightOperationWidget::new(label.clone()));
                    Box::new(RowWidget::new(body).with_accent(accent_style))
                }
                ChatItemData::SlashInput { raw, .. } => {
                    let body = Box::new(SlashInputWidget::new(raw.clone()));
                    Box::new(RowWidget::new(body).with_accent(accent_style))
                }
                ChatItemData::TuiCommandResponse {
                    command, response, ..
                } => {
                    let body = Box::new(CommandResponseWidget::new(
                        format!("/{command}"),
                        response.clone().into(),
                    ));
                    Box::new(RowWidget::new(body).with_accent(accent_style))
                }
                _ => unreachable!("All meta items should be handled"),
            }
        }
    }
}

#[expect(dead_code)]
pub fn format_command_response(resp: &CommandResponse) -> String {
    match resp {
        CommandResponse::Text(text) => text.clone(),
        CommandResponse::Compact(result) => match result {
            CompactResult::Success(_) => "Compaction complete.".to_string(),
            CompactResult::Failed(error) => format!("Compaction failed: {error}"),
            CompactResult::Cancelled => "Compact cancelled.".to_string(),
            CompactResult::InsufficientMessages => "Not enough messages to compact.".to_string(),
        },
    }
}

fn build_edited_message_ids(chat_store: &ChatStore) -> HashSet<String> {
    let mut edited_message_ids = HashSet::new();
    let mut seen_by_parent: HashMap<Option<String>, String> = HashMap::new();

    for item in chat_store.iter_items() {
        let ChatItemData::Message(message) = &item.data else {
            continue;
        };
        if !matches!(message.data, MessageData::User { .. }) {
            continue;
        }

        let parent_id = message.parent_message_id().map(|id| id.to_string());
        if let std::collections::hash_map::Entry::Vacant(e) = seen_by_parent.entry(parent_id) {
            e.insert(message.id().to_string());
        } else {
            edited_message_ids.insert(message.id().to_string());
        }
    }

    edited_message_ids
}

/// Build the lineage set by following parent_message_id chain backwards from active_message_id
fn build_lineage_set(active_message_id: &str, chat_store: &ChatStore) -> HashSet<String> {
    let mut lineage = HashSet::new();
    let mut current = Some(active_message_id.to_string());

    while let Some(id) = current {
        lineage.insert(id.clone());

        // Get the parent_message_id of the current message
        current = chat_store.get_by_id(&id).and_then(|item| {
            if let ChatItemData::Message(msg) = &item.data {
                msg.parent_message_id().map(|s| s.to_string())
            } else {
                None
            }
        });
    }

    lineage
}

fn build_visible_message_set(lineage: &HashSet<String>, chat_store: &ChatStore) -> HashSet<String> {
    let mut visible_messages = lineage.clone();
    let mut pending_summaries: Vec<String> = lineage
        .iter()
        .filter(|id| chat_store.compacted_head_for_summary(id).is_some())
        .cloned()
        .collect();
    let mut expanded_summaries = HashSet::new();

    while let Some(summary_id) = pending_summaries.pop() {
        if !expanded_summaries.insert(summary_id.clone()) {
            continue;
        }

        let Some(compacted_head_id) = chat_store.compacted_head_for_summary(&summary_id) else {
            continue;
        };

        let mut current_id = Some(compacted_head_id.to_string());
        while let Some(id) = current_id {
            let inserted = visible_messages.insert(id.clone());

            // If we encounter another summary while walking history, enqueue it so
            // we recursively include the messages compacted by earlier rounds.
            if chat_store.compacted_head_for_summary(&id).is_some() {
                pending_summaries.push(id.clone());
            }

            if !inserted {
                break;
            }

            current_id = chat_store.get_by_id(&id).and_then(|item| {
                if let ChatItemData::Message(msg) = &item.data {
                    msg.parent_message_id().map(str::to_string)
                } else {
                    None
                }
            });
        }
    }

    visible_messages
}

/// Check if a ChatItem should be visible based on the visible messages set
fn is_visible(item: &ChatItem, visible_messages: &HashSet<String>, chat_store: &ChatStore) -> bool {
    if let ChatItemData::Message(msg) = &item.data {
        return visible_messages.contains(msg.id());
    }

    // Root-level meta/tool rows (no parent) are always visible
    if item.parent_chat_item_id.is_none() {
        return true;
    }

    let mut current = item.parent_chat_item_id.as_deref();
    while let Some(parent_id) = current {
        if visible_messages.contains(parent_id) {
            return true;
        }

        // Check if this parent is a message - if so, we've already checked visibility
        if let Some(parent_item) = chat_store.get_by_id(&parent_id.to_string()) {
            if matches!(parent_item.data, ChatItemData::Message(_)) {
                // If we reached a message and it's not visible, stop here
                return false;
            }
            // Otherwise continue walking up
            current = parent_item.parent_chat_item_id.as_deref();
        } else {
            // Parent doesn't exist, stop
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::NoticeLevel;
    use crate::tui::state::chat_store::ChatStore;
    use ratatui::{Terminal, backend::TestBackend};
    use std::time::SystemTime;
    use steer_grpc::client_api::{AssistantContent, Message, MessageData, UserContent};
    use steer_tools::result::ExternalResult;

    fn create_test_message(content: &str, id: &str) -> ChatItem {
        ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::Message(Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: content.to_string(),
                    }],
                },
                timestamp: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                id: id.to_string(),
                parent_message_id: None,
            }),
        }
    }

    fn create_assistant_message(content: &str, id: &str) -> ChatItem {
        ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::Message(Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: content.to_string(),
                    }],
                },
                timestamp: SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                id: id.to_string(),
                parent_message_id: None,
            }),
        }
    }

    fn create_test_chat_store() -> ChatStore {
        ChatStore::default()
    }

    #[test]
    fn test_partial_rendering_with_large_first_item() {
        // Create a viewport with a very large first item
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        // Create test messages - first one with many lines
        let mut messages = vec![];

        // First message with 10 lines of content
        let large_content = (0..10)
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
                let chat_store = create_test_chat_store();
                viewport.rebuild(
                    &messages.iter().collect::<Vec<_>>(),
                    area.width,
                    ViewMode::Compact,
                    &theme,
                    &chat_store,
                    None,
                );

                // Render the viewport
                viewport.render(f, area, &theme);
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

        // We should see at least the second message
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
                let chat_store = create_test_chat_store();
                viewport.rebuild(
                    &messages.iter().collect::<Vec<_>>(),
                    area.width,
                    ViewMode::Compact,
                    &theme,
                    &chat_store,
                    None,
                );

                // Measure visible rows
                let _rows = viewport.measure_visible_rows(area);
                let total_height = viewport.state.total_content_height;
                println!("Total content height: {}, viewport height: {}", total_height, area.height);

                // Debug: print individual item heights
                for (idx, item) in viewport.items.iter().enumerate() {
                    let height = item.cached_heights.get_height(ViewMode::Compact).unwrap_or(0);
                    println!("Item {idx} height: {height}");
                }

                assert!(total_height > area.height as usize, "Content should be taller than viewport for this test - total_height: {}, viewport_height: {}", total_height, area.height);

                // Scroll to bottom
                viewport.state.scroll_to_bottom();

                // Render
                viewport.render(f, area, &theme);
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
                .map(|x| buffer.cell((x, y)).map_or(" ", |c| c.symbol()))
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
            let chat_store = create_test_chat_store();
            viewport.rebuild(
                &messages.iter().collect::<Vec<_>>(),
                area.width,
                ViewMode::Compact,
                &theme,
                &chat_store,
                None,
            );

            let _rows = viewport.measure_visible_rows(area);
            let total_height = viewport.state.total_content_height;
            println!("Total content height: {}, viewport height: {}", total_height, area.height);

            // Debug: print individual item heights
            for (idx, item) in viewport.items.iter().enumerate() {
            let height = item.cached_heights.get_height(ViewMode::Compact).unwrap_or(0);
            println!("Item {idx} height: {height}");
            }

            assert!(total_height > area.height as usize, "Content should be much taller than viewport - total_height: {}, viewport_height: {}", total_height, area.height);

            // Scroll to bottom
            viewport.state.scroll_to_bottom();

            // Render
            viewport.render(f, area, &theme);
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

        let chat_store = create_test_chat_store();

        // Rebuild viewport
        viewport.rebuild(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
        );

        // Scroll to top
        viewport.state.offset = 0;

        // Measure visible rows
        let rows = viewport.measure_visible_rows(area);

        // Should see all messages if they fit
        assert!(!rows.is_empty(), "Should have visible rows");
        match rows[0].kind {
            RowKind::Item { idx } => {
                assert_eq!(idx, 0, "First visible row should be index 0");
            }
            RowKind::Gap => {
                panic!("First visible row should be a message row");
            }
        }
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

        let chat_store = create_test_chat_store();

        // Rebuild viewport
        viewport.rebuild(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
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
        let rows = viewport.measure_visible_rows(area);

        assert!(!rows.is_empty(), "Should have visible rows");
        match rows[0].kind {
            RowKind::Item { idx } => {
                assert_eq!(idx, 0, "First visible row should still be index 0");
            }
            RowKind::Gap => {
                panic!("First visible row should be a message row");
            }
        }

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

    #[test]
    fn test_gap_rows_render_when_offset_in_spacing() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let messages = vec![
            create_assistant_message("First message", "msg1"),
            create_assistant_message("Second message", "msg2"),
        ];

        let backend = TestBackend::new(80, 2);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|f| {
                let area = f.area();

                let chat_store = create_test_chat_store();
                viewport.rebuild_for_test(
                    &messages.iter().collect::<Vec<_>>(),
                    area.width,
                    ViewMode::Compact,
                    &theme,
                    &chat_store,
                    None,
                    2,
                );

                let _rows = viewport.measure_visible_rows(area);
                let first_height = viewport.items[0]
                    .cached_heights
                    .get_height(ViewMode::Compact)
                    .unwrap_or(0);
                assert!(first_height > 0, "First message should have height");

                // Offset into the spacing segment so we render one gap row and one content row.
                viewport.state.offset = first_height.saturating_add(1);

                viewport.render(f, area, &theme);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let line0: String = (0..80)
            .map(|x| buffer.cell((x, 0)).map_or(" ", |c| c.symbol()))
            .collect();
        let line1: String = (0..80)
            .map(|x| buffer.cell((x, 1)).map_or(" ", |c| c.symbol()))
            .collect();

        assert!(
            !line0.contains("Second message"),
            "First visible line should be the gap row"
        );
        assert!(
            line1.contains("Second message"),
            "Second line should contain the next message"
        );
    }

    #[test]
    fn test_gap_rows_in_measure_visible_rows() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let messages = vec![
            create_assistant_message("First message", "msg1"),
            create_assistant_message("Second message", "msg2"),
        ];

        let area = Rect::new(0, 0, 80, 3);
        let chat_store = create_test_chat_store();

        viewport.rebuild_for_test(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
            2,
        );

        let _rows = viewport.measure_visible_rows(area);
        let first_height = viewport.items[0]
            .cached_heights
            .get_height(ViewMode::Compact)
            .unwrap_or(0);

        viewport.state.offset = first_height;
        let rows = viewport.measure_visible_rows(area);

        assert!(
            rows.iter().any(|row| matches!(row.kind, RowKind::Gap)),
            "Gap rows should be represented when offset is inside spacing"
        );
    }

    #[test]
    fn test_total_height_includes_spacing() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let messages = vec![
            create_assistant_message("First message", "msg1"),
            create_assistant_message("Second message", "msg2"),
        ];

        let area = Rect::new(0, 0, 80, 10);
        let chat_store = create_test_chat_store();

        viewport.rebuild_for_test(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
            2,
        );

        let _rows = viewport.measure_visible_rows(area);
        let h1 = viewport.items[0]
            .cached_heights
            .get_height(ViewMode::Compact)
            .unwrap_or(0);
        let h2 = viewport.items[1]
            .cached_heights
            .get_height(ViewMode::Compact)
            .unwrap_or(0);
        let spacing = 2usize;

        assert_eq!(
            viewport.state.total_content_height,
            h1.saturating_add(spacing).saturating_add(h2),
            "Total height should include spacing between message rows"
        );
    }

    #[test]
    fn test_toggle_view_mode_rebuilds_heights_for_mode_specific_widgets() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();
        let chat_store = create_test_chat_store();
        let area = Rect::new(0, 0, 80, 10);

        let items = vec![ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::PendingToolCall {
                id: "pending_edit".to_string(),
                tool_call: ToolCall {
                    id: "call_edit".to_string(),
                    name: "edit".to_string(),
                    parameters: serde_json::json!({
                        "file_path": "/tmp/test.rs",
                        "old_string": "fn old_name() {\n    println!(\"before\");\n}",
                        "new_string": "fn new_name() {\n    println!(\"after\");\n    println!(\"extra\");\n}",
                    }),
                },
                ts: time::OffsetDateTime::now_utc(),
            },
        }];
        let raw = items.iter().collect::<Vec<_>>();

        viewport.rebuild(
            &raw,
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
        );
        let compact_height = viewport.state.total_content_height;

        let mut expected = ChatViewport::new();
        expected.rebuild(
            &raw,
            area.width,
            ViewMode::Detailed,
            &theme,
            &chat_store,
            None,
        );
        let detailed_height = expected.state.total_content_height;
        assert_ne!(
            compact_height, detailed_height,
            "Test setup requires compact and detailed heights to differ"
        );

        viewport.state_mut().toggle_view_mode();
        viewport.state_mut().scroll_to_bottom();
        let mode_after_toggle = viewport.state().view_mode;
        viewport.rebuild(
            &raw,
            area.width,
            mode_after_toggle,
            &theme,
            &chat_store,
            None,
        );

        assert_eq!(
            viewport.state.total_content_height, detailed_height,
            "Toggling view mode should rebuild item heights for the new mode"
        );
        assert!(
            viewport.items[0]
                .cached_heights
                .get_height(ViewMode::Detailed)
                .is_some(),
            "Detailed height cache should be populated after toggling modes"
        );
    }

    #[test]
    fn test_scroll_to_item_out_of_range_clamps() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let large_content = (0..20)
            .map(|i| format!("Line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let messages = vec![
            create_assistant_message(&large_content, "msg1"),
            create_assistant_message("Second message", "msg2"),
        ];

        let area = Rect::new(0, 0, 80, 5);
        let chat_store = create_test_chat_store();

        viewport.rebuild(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
        );

        let _rows = viewport.measure_visible_rows(area);
        let max_offset = viewport
            .state
            .total_content_height
            .saturating_sub(area.height as usize);

        viewport.state.scroll_to_item(usize::MAX);
        let _rows = viewport.measure_visible_rows(area);

        assert!(
            viewport.state.offset <= max_offset,
            "Offset should stay clamped even for invalid item targets"
        );
    }

    #[test]
    fn test_visible_range_tracks_item_rows_only() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let messages = vec![
            create_assistant_message("First", "msg1"),
            create_assistant_message("Second", "msg2"),
            create_assistant_message("Third", "msg3"),
        ];

        let area = Rect::new(0, 0, 80, 3);
        let chat_store = create_test_chat_store();

        viewport.rebuild(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
        );

        // Offset into the first gap row so the first visible content row is item 1.
        viewport.state.offset = 1;
        let rows = viewport.measure_visible_rows(area);
        assert!(!rows.is_empty(), "Expected visible rows");

        let visible = viewport
            .state
            .visible_range
            .clone()
            .expect("visible range should be populated");
        assert_eq!(visible.first_index, 1);
        assert_eq!(visible.last_index, 1);
        assert_eq!(visible.first_y, 1);
        assert_eq!(visible.last_y, 1);
    }

    #[test]
    fn test_measure_visible_rows_jumps_to_first_visible_segment() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let large_content = (0..200)
            .map(|i| format!("Large line {i}"))
            .collect::<Vec<_>>()
            .join("\n");

        let messages = vec![
            create_assistant_message(&large_content, "msg1"),
            create_assistant_message("Second message", "msg2"),
            create_assistant_message("Third message", "msg3"),
        ];

        let area = Rect::new(0, 0, 80, 2);
        let chat_store = create_test_chat_store();

        viewport.rebuild(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
        );

        let _rows = viewport.measure_visible_rows(area);
        let first_height = viewport.items[0]
            .cached_heights
            .get_height(ViewMode::Compact)
            .expect("first item should have cached height");

        // Start exactly at the second item boundary.
        viewport.state.offset = first_height.saturating_add(1);
        let rows = viewport.measure_visible_rows(area);
        let first_item = rows.iter().find_map(|row| match row.kind {
            RowKind::Item { idx } => Some(idx),
            RowKind::Gap => None,
        });

        assert_eq!(first_item, Some(1));
    }

    #[test]
    fn test_scroll_to_item_centers_using_indexed_start_positions() {
        let mut viewport = ChatViewport::new();
        let theme = Theme::default();

        let messages = vec![
            create_assistant_message("First", "msg1"),
            create_assistant_message("Second", "msg2"),
            create_assistant_message("Third", "msg3"),
        ];

        let area = Rect::new(0, 0, 80, 4);
        let chat_store = create_test_chat_store();

        viewport.rebuild_for_test(
            &messages.iter().collect::<Vec<_>>(),
            area.width,
            ViewMode::Compact,
            &theme,
            &chat_store,
            None,
            2,
        );

        let _rows = viewport.measure_visible_rows(area);
        let expected = viewport.item_start_y[1].saturating_sub((area.height as usize) / 2);

        viewport.state.scroll_to_item(1);
        let _rows = viewport.measure_visible_rows(area);

        assert_eq!(viewport.state.offset, expected);
    }

    #[test]
    fn test_build_lineage_set_basic_chain() {
        let mut store = create_test_chat_store();

        // Create a chain: root -> A -> B -> C
        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Root message".to_string(),
                }],
            },
            id: "root".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::Assistant { content: vec![] },
            id: "A".to_string(),
            timestamp: 1001,
            parent_message_id: Some("root".to_string()),
        });

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message B".to_string(),
                }],
            },
            id: "B".to_string(),
            timestamp: 1002,
            parent_message_id: Some("A".to_string()),
        });

        store.add_message(Message {
            data: MessageData::Assistant { content: vec![] },
            id: "C".to_string(),
            timestamp: 1003,
            parent_message_id: Some("B".to_string()),
        });

        // Build lineage from C
        let lineage = build_lineage_set("C", &store);
        assert_eq!(
            lineage,
            HashSet::from([
                "C".to_string(),
                "B".to_string(),
                "A".to_string(),
                "root".to_string(),
            ])
        );
    }

    #[test]
    fn test_build_lineage_set_single_message() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Single message".to_string(),
                }],
            },
            id: "single".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        let lineage = build_lineage_set("single", &store);
        assert_eq!(lineage, HashSet::from(["single".to_string()]));
    }

    #[test]
    fn test_build_lineage_set_invalid_id() {
        let store = create_test_chat_store();

        let lineage = build_lineage_set("nonexistent", &store);

        assert_eq!(lineage, HashSet::from(["nonexistent".to_string()]));
    }

    #[test]
    fn test_build_visible_message_set_includes_pre_compaction_chain() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "pre user".to_string(),
                }],
            },
            id: "pre_u".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "pre assistant".to_string(),
                }],
            },
            id: "pre_a".to_string(),
            timestamp: 1001,
            parent_message_id: Some("pre_u".to_string()),
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "summary".to_string(),
                }],
            },
            id: "summary".to_string(),
            timestamp: 1002,
            parent_message_id: None,
        });
        store.mark_compaction_summary_with_head("summary".to_string(), Some("pre_a".to_string()));

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "post user".to_string(),
                }],
            },
            id: "post_u".to_string(),
            timestamp: 1003,
            parent_message_id: Some("summary".to_string()),
        });
        store.set_active_message_id(Some("post_u".to_string()));

        let lineage = build_lineage_set("post_u", &store);
        let visible = build_visible_message_set(&lineage, &store);

        assert!(visible.contains("summary"));
        assert!(visible.contains("post_u"));
        assert!(visible.contains("pre_a"));
        assert!(visible.contains("pre_u"));
    }

    #[test]
    fn test_build_visible_message_set_nested_compaction_summaries() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "pre1 user".to_string(),
                }],
            },
            id: "pre1_u".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });
        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "pre1 assistant".to_string(),
                }],
            },
            id: "pre1_a".to_string(),
            timestamp: 1001,
            parent_message_id: Some("pre1_u".to_string()),
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "summary1".to_string(),
                }],
            },
            id: "summary1".to_string(),
            timestamp: 1002,
            parent_message_id: None,
        });
        store.mark_compaction_summary_with_head("summary1".to_string(), Some("pre1_a".to_string()));

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "mid user".to_string(),
                }],
            },
            id: "mid_u".to_string(),
            timestamp: 1003,
            parent_message_id: Some("summary1".to_string()),
        });
        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "mid assistant".to_string(),
                }],
            },
            id: "mid_a".to_string(),
            timestamp: 1004,
            parent_message_id: Some("mid_u".to_string()),
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "summary2".to_string(),
                }],
            },
            id: "summary2".to_string(),
            timestamp: 1005,
            parent_message_id: None,
        });
        store.mark_compaction_summary_with_head("summary2".to_string(), Some("mid_a".to_string()));

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "post user".to_string(),
                }],
            },
            id: "post_u".to_string(),
            timestamp: 1006,
            parent_message_id: Some("summary2".to_string()),
        });

        let lineage = build_lineage_set("post_u", &store);
        let visible = build_visible_message_set(&lineage, &store);

        assert!(visible.contains("pre1_u"));
        assert!(visible.contains("pre1_a"));
        assert!(visible.contains("summary1"));
        assert!(visible.contains("mid_u"));
        assert!(visible.contains("mid_a"));
        assert!(visible.contains("summary2"));
        assert!(visible.contains("post_u"));
    }

    #[test]
    fn test_build_visible_message_set_without_compaction_mapping_matches_lineage() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "root".to_string(),
                }],
            },
            id: "root".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::Assistant { content: vec![] },
            id: "leaf".to_string(),
            timestamp: 1001,
            parent_message_id: Some("root".to_string()),
        });

        let lineage = build_lineage_set("leaf", &store);
        let visible = build_visible_message_set(&lineage, &store);

        assert_eq!(visible, lineage);
    }

    #[test]
    fn test_compaction_history_visibility_filters_other_branches() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "pre root".to_string(),
                }],
            },
            id: "pre_root".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "pre assistant".to_string(),
                }],
            },
            id: "pre_a".to_string(),
            timestamp: 1001,
            parent_message_id: Some("pre_root".to_string()),
        });

        store.add_message(Message {
            data: MessageData::Assistant {
                content: vec![AssistantContent::Text {
                    text: "summary".to_string(),
                }],
            },
            id: "summary".to_string(),
            timestamp: 1002,
            parent_message_id: None,
        });
        store.mark_compaction_summary_with_head("summary".to_string(), Some("pre_a".to_string()));

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "active branch".to_string(),
                }],
            },
            id: "active_u".to_string(),
            timestamp: 1003,
            parent_message_id: Some("summary".to_string()),
        });

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "other branch".to_string(),
                }],
            },
            id: "other_u".to_string(),
            timestamp: 1004,
            parent_message_id: Some("pre_root".to_string()),
        });

        let raw = store.as_vec();
        let lineage = build_lineage_set("active_u", &store);
        let visible = build_visible_message_set(&lineage, &store);
        let visible_ids: HashSet<String> = raw
            .iter()
            .filter_map(|item| {
                if is_visible(item, &visible, &store)
                    && let ChatItemData::Message(message) = &item.data
                {
                    return Some(message.id().to_string());
                }
                None
            })
            .collect();

        assert!(visible_ids.contains("pre_root"));
        assert!(visible_ids.contains("pre_a"));
        assert!(visible_ids.contains("summary"));
        assert!(visible_ids.contains("active_u"));
        assert!(!visible_ids.contains("other_u"));
    }

    #[test]
    fn test_is_visible_message_in_lineage() {
        let mut store = create_test_chat_store();

        // Create messages
        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message 1".to_string(),
                }],
            },
            id: "msg1".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::Assistant { content: vec![] },
            id: "msg2".to_string(),
            timestamp: 1001,
            parent_message_id: Some("msg1".to_string()),
        });

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message 3".to_string(),
                }],
            },
            id: "msg3".to_string(),
            timestamp: 1002,
            parent_message_id: Some("msg1".to_string()), // Branch from msg1
        });

        let lineage = build_lineage_set("msg2", &store);
        let visible = build_visible_message_set(&lineage, &store);
        assert_eq!(
            visible,
            HashSet::from(["msg2".to_string(), "msg1".to_string(),])
        );

        let lineage = build_lineage_set("msg3", &store);
        let visible = build_visible_message_set(&lineage, &store);
        assert_eq!(
            visible,
            HashSet::from(["msg3".to_string(), "msg1".to_string(),])
        );
    }

    #[test]
    fn test_is_visible_root_meta_items() {
        let mut store = create_test_chat_store();
        let visible_messages = HashSet::new(); // Empty visible set

        // Add root-level system notice
        let notice = ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::SystemNotice {
                id: "notice1".to_string(),
                level: NoticeLevel::Info,
                text: "System notice".to_string(),
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(notice.clone());

        // Root-level items should always be visible
        assert!(is_visible(&notice, &visible_messages, &store));
    }

    #[test]
    fn test_is_visible_attached_meta_items() {
        let mut store = create_test_chat_store();

        // Create messages
        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message 1".to_string(),
                }],
            },
            id: "msg1".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message 2".to_string(),
                }],
            },
            id: "msg2".to_string(),
            timestamp: 1001,
            parent_message_id: None,
        });

        // Add tool call attached to msg1
        let tool_call1 = ChatItem {
            parent_chat_item_id: Some("msg1".to_string()),
            data: ChatItemData::PendingToolCall {
                id: "tool1".to_string(),
                tool_call: ToolCall {
                    id: "call1".to_string(),
                    name: "test_tool".to_string(),
                    parameters: serde_json::Value::String("{}".to_string()),
                },
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(tool_call1.clone());

        // Add tool call attached to msg2
        let tool_call2 = ChatItem {
            parent_chat_item_id: Some("msg2".to_string()),
            data: ChatItemData::PendingToolCall {
                id: "tool2".to_string(),
                tool_call: ToolCall {
                    id: "call2".to_string(),
                    name: "test_tool".to_string(),
                    parameters: serde_json::Value::String("{}".to_string()),
                },
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(tool_call2.clone());

        // Build visible set from msg1 lineage
        let lineage = build_lineage_set("msg1", &store);
        let visible_messages = build_visible_message_set(&lineage, &store);

        // tool_call1 should be visible (attached to msg1)
        assert!(is_visible(&tool_call1, &visible_messages, &store));

        // tool_call2 should NOT be visible (attached to msg2 which is not visible)
        assert!(!is_visible(&tool_call2, &visible_messages, &store));
    }

    #[test]
    fn test_is_visible_nested_attachments() {
        let mut store = create_test_chat_store();

        // Create a message
        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message 1".to_string(),
                }],
            },
            id: "msg1".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        // Add a system notice attached to the message
        let notice = ChatItem {
            parent_chat_item_id: Some("msg1".to_string()),
            data: ChatItemData::SystemNotice {
                id: "notice1".to_string(),
                level: NoticeLevel::Info,
                text: "Notice attached to msg1".to_string(),
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(notice);

        // Add another notice attached to the first notice (nested)
        let nested_notice = ChatItem {
            parent_chat_item_id: Some("notice1".to_string()),
            data: ChatItemData::SystemNotice {
                id: "notice2".to_string(),
                level: NoticeLevel::Info,
                text: "Notice attached to notice1".to_string(),
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(nested_notice.clone());

        let lineage = build_lineage_set("msg1", &store);
        let visible_messages = build_visible_message_set(&lineage, &store);

        // First notice should be visible (attached to msg1)
        let notice1 = store.get_by_id(&"notice1".to_string()).unwrap();
        assert!(
            is_visible(notice1, &visible_messages, &store),
            "notice1 should be visible"
        );

        // Nested notice should be visible through the chain
        assert!(
            is_visible(&nested_notice, &visible_messages, &store),
            "nested_notice should be visible"
        );
    }

    #[test]
    fn test_branch_filtering_integration() {
        let mut store = create_test_chat_store();

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message A".to_string(),
                }],
            },
            id: "A".to_string(),
            timestamp: 1000,
            parent_message_id: None,
        });

        // Add system notice (no parent)
        let root_notice = ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::SystemNotice {
                id: "notice_root".to_string(),
                level: NoticeLevel::Info,
                text: "Root notice".to_string(),
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(root_notice.clone());

        store.add_message(Message {
            data: MessageData::Assistant { content: vec![] },
            id: "B".to_string(),
            timestamp: 1001,
            parent_message_id: Some("A".to_string()),
        });

        // Add tool call attached to B
        let pending_tool_b = ChatItem {
            parent_chat_item_id: Some("B".to_string()),
            data: ChatItemData::PendingToolCall {
                id: "tool_b".to_string(),
                tool_call: ToolCall {
                    id: "call_b".to_string(),
                    name: "tool_b".to_string(),
                    parameters: serde_json::Value::String("{}".to_string()),
                },
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(pending_tool_b.clone());

        let tool_b_message = ChatItem {
            parent_chat_item_id: None,
            data: ChatItemData::Message(Message {
                data: MessageData::Tool {
                    tool_use_id: "call_b".to_string(),
                    result: ToolResult::External(ExternalResult {
                        tool_name: "tool_b".to_string(),
                        payload: String::new(),
                    }),
                },
                timestamp: 0,
                id: "tool_b".to_string(),
                parent_message_id: Some("B".to_string()),
            }),
        };
        store.push(tool_b_message.clone());

        store.add_message(Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "Message C".to_string(),
                }],
            },
            id: "C".to_string(),
            timestamp: 1002,
            parent_message_id: Some("tool_b".to_string()),
        });

        // Add tool call attached to C
        let tool_c = ChatItem {
            parent_chat_item_id: Some("C".to_string()),
            data: ChatItemData::PendingToolCall {
                id: "tool_c".to_string(),
                tool_call: ToolCall {
                    id: "call_c".to_string(),
                    name: "tool_c".to_string(),
                    parameters: serde_json::Value::String("{}".to_string()),
                },
                ts: time::OffsetDateTime::now_utc(),
            },
        };
        store.push(tool_c.clone());

        // Test with active_message_id = B
        let lineage_b = build_lineage_set("B", &store);
        let visible_b = build_visible_message_set(&lineage_b, &store);

        // Should see: root notice, A, B, B's tool call
        assert!(is_visible(&root_notice, &visible_b, &store));
        assert!(is_visible(
            store.get_by_id(&"A".to_string()).unwrap(),
            &visible_b,
            &store
        ));
        assert!(is_visible(
            store.get_by_id(&"B".to_string()).unwrap(),
            &visible_b,
            &store
        ));
        assert!(is_visible(&pending_tool_b, &visible_b, &store));

        // Should NOT see: C, C's tool call
        assert!(!is_visible(
            store.get_by_id(&"C".to_string()).unwrap(),
            &visible_b,
            &store
        ));
        assert!(!is_visible(&tool_c, &visible_b, &store));

        // Test with active_message_id = C
        let lineage_c = build_lineage_set("C", &store);
        let visible_c = build_visible_message_set(&lineage_c, &store);

        assert_eq!(
            visible_c,
            HashSet::from([
                "A".to_string(),
                "B".to_string(),
                "tool_b".to_string(),
                "C".to_string(),
            ])
        );

        // Should see: root notice, A, C, C's tool call
        assert!(is_visible(&root_notice, &visible_c, &store));
        assert!(is_visible(
            store.get_by_id(&"A".to_string()).unwrap(),
            &visible_c,
            &store
        ));
        assert!(is_visible(
            store.get_by_id(&"C".to_string()).unwrap(),
            &visible_c,
            &store
        ));
        assert!(is_visible(&tool_c, &visible_c, &store));

        // Should NOT see: B, B's tool call
        assert!(is_visible(
            store.get_by_id(&"B".to_string()).unwrap(),
            &visible_c,
            &store
        ));
        assert!(is_visible(&pending_tool_b, &visible_c, &store));
    }
}
