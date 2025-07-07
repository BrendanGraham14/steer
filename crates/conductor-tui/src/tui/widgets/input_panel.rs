use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, StatefulWidget, Widget};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use tui_textarea::{Input, TextArea};

use conductor_core::app::conversation::{Message, UserContent};
use conductor_tools::schema::ToolCall;

use crate::tui::InputMode;
use crate::tui::get_spinner_char;
use crate::tui::model::ChatItem;
use crate::tui::state::file_cache::FileCache;
use crate::tui::widgets::fuzzy_finder::FuzzyFinder;

/// Stateful data for the [`InputPanel`] widget.
#[derive(Debug)]
pub struct InputPanelState {
    pub textarea: TextArea<'static>,
    pub edit_selection_messages: Vec<(String, String)>,
    pub edit_selection_index: usize,
    pub edit_selection_hovered_id: Option<String>,
    /// File cache for fuzzy finder
    pub file_cache: FileCache,
    /// Fuzzy finder widget
    pub fuzzy_finder: FuzzyFinder,
}

impl Default for InputPanelState {
    fn default() -> Self {
        // For tests and default usage, use a dummy session ID
        Self::new("default".to_string())
    }
}

impl InputPanelState {
    /// Create a new InputPanelState with the given session ID
    pub fn new(session_id: String) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Type your message here...");
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        Self {
            textarea,
            edit_selection_messages: Vec::new(),
            edit_selection_index: 0,
            edit_selection_hovered_id: None,
            file_cache: FileCache::new(session_id),
            fuzzy_finder: FuzzyFinder::new(),
        }
    }

    /// Get the byte offset of the cursor in the textarea content.
    pub fn get_cursor_byte_offset(&self) -> usize {
        let (row, col) = self.textarea.cursor();
        let lines = self.textarea.lines();
        let mut offset = 0;
        for (i, line) in lines.iter().enumerate() {
            if i < row {
                offset += line.len() + 1; // +1 for newline
            } else {
                // `col` is a grapheme cluster count, find the byte offset for that.
                offset += line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
                break;
            }
        }
        offset
    }

    /// Checks if the fuzzy finder is active and the cursor is in a valid query position.
    /// This method does not allocate and is suitable for checks on every tick.
    pub fn is_in_fuzzy_query(&self) -> bool {
        if !self.fuzzy_finder.is_active() {
            return false;
        }

        let Some(at_pos) = self.fuzzy_finder.trigger_position() else {
            return false;
        };

        let cursor_offset = self.get_cursor_byte_offset();
        if cursor_offset <= at_pos {
            return false; // Cursor is before or on the trigger character
        }

        let content = self.content();
        // The part of the string that could be the query
        let query_candidate = &content[at_pos + 1..cursor_offset];

        // If it contains whitespace, it's not a valid query anymore
        !query_candidate.chars().any(char::is_whitespace)
    }

    /// If the fuzzy finder is active and the cursor is in a valid query position,
    /// returns the query string. Otherwise, returns None.
    pub fn get_current_fuzzy_query(&self) -> Option<String> {
        if self.is_in_fuzzy_query() {
            let at_pos = self.fuzzy_finder.trigger_position().unwrap(); // Safe due to check above
            let cursor_offset = self.get_cursor_byte_offset();
            let content = self.content();
            let query_candidate = &content[at_pos + 1..cursor_offset];
            Some(query_candidate.to_string())
        } else {
            None
        }
    }

    /// Handle input in insert/bash modes
    pub fn handle_input(&mut self, input: Input) {
        self.textarea.input(input);
    }

    /// Complete fuzzy finder by replacing the query text with the selected path
    pub fn complete_fuzzy_finder(&mut self, selected_path: &str) {
        if let Some(at_pos) = self.fuzzy_finder.trigger_position() {
            let cursor_offset = self.get_cursor_byte_offset();

            // Convert content to string and replace the query portion
            let content = self.content();
            let mut new_content = String::new();

            // Keep everything up to and including the @
            new_content.push_str(&content[..=at_pos]);

            // Add the selected path and a space
            new_content.push_str(selected_path);
            new_content.push(' ');

            // Keep everything after the cursor
            if cursor_offset < content.len() {
                new_content.push_str(&content[cursor_offset..]);
            }

            // Replace the entire content
            let lines: Vec<&str> = new_content.lines().collect();
            self.set_content_from_lines(lines);

            // Position cursor after the inserted path and space (which is a byte position)
            let new_cursor_pos_bytes = at_pos + 1 + selected_path.len() + 1;

            // Now, convert this byte position to a (row, col) grapheme position
            let mut bytes_traversed = 0;
            for (row_idx, line) in self.textarea.lines().iter().enumerate() {
                let line_len_bytes = line.len();
                if bytes_traversed + line_len_bytes >= new_cursor_pos_bytes {
                    // The cursor should be on this line
                    let byte_offset_in_line = new_cursor_pos_bytes - bytes_traversed;
                    // Convert byte offset in line to character/grapheme column
                    let char_col = line[..byte_offset_in_line].chars().count();
                    self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
                        row_idx as u16,
                        char_col as u16,
                    ));
                    break;
                }
                bytes_traversed += line_len_bytes + 1; // +1 for newline
            }
        }
    }

    /// Insert a string (e.g., for paste operations)
    pub fn insert_str(&mut self, s: &str) {
        self.textarea.insert_str(s);
    }

    /// Get the current content as a single string
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Clear the textarea
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Type your message here...");
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea
            .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    }

    /// Set content from lines (used when editing a message)
    pub fn set_content_from_lines(&mut self, lines: Vec<&str>) {
        self.textarea = TextArea::from(lines);
    }

    /// Calculate required height for the input panel
    pub fn required_height(&self, max_height: u16) -> u16 {
        let line_count = self.textarea.lines().len().max(1);
        // line count + 2 for borders + 1 for padding
        (line_count + 3).min(max_height as usize) as u16
    }

    /// Calculate required height for approval mode
    pub fn required_height_for_approval(tool_call: &ToolCall, width: u16, max_height: u16) -> u16 {
        let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
        let preview_lines = formatter.compact(
            &tool_call.parameters,
            &None,
            width.saturating_sub(4) as usize,
        );
        // 2 lines for header + preview lines + 2 for borders + 1 for padding
        (2 + preview_lines.len() + 3).min(max_height as usize) as u16
    }

    /// Navigate up in edit selection mode
    pub fn edit_selection_prev(&mut self) -> Option<&(String, String)> {
        if self.edit_selection_index > 0 {
            self.edit_selection_index -= 1;
            self.update_hovered_id();
            self.edit_selection_messages.get(self.edit_selection_index)
        } else {
            self.edit_selection_messages.get(self.edit_selection_index)
        }
    }

    /// Navigate down in edit selection mode
    pub fn edit_selection_next(&mut self) -> Option<&(String, String)> {
        if self.edit_selection_index + 1 < self.edit_selection_messages.len() {
            self.edit_selection_index += 1;
            self.update_hovered_id();
            self.edit_selection_messages.get(self.edit_selection_index)
        } else {
            self.edit_selection_messages.get(self.edit_selection_index)
        }
    }

    /// Get currently selected message in edit selection mode
    pub fn get_selected_message(&self) -> Option<&(String, String)> {
        self.edit_selection_messages.get(self.edit_selection_index)
    }

    /// Populate edit selection messages from chat store
    pub fn populate_edit_selection(&mut self, chat_items: &[ChatItem]) {
        self.edit_selection_messages = chat_items
            .iter()
            .filter_map(|item| {
                if let ChatItem::Message(row) = item {
                    if let Message::User { content, .. } = &row.inner {
                        // Extract text content from user blocks
                        let text = content
                            .iter()
                            .filter_map(|block| match block {
                                UserContent::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Some((row.inner.id().to_string(), text))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Select the last (most recent) message if available
        if !self.edit_selection_messages.is_empty() {
            self.edit_selection_index = self.edit_selection_messages.len() - 1;
            self.update_hovered_id();
        } else {
            self.edit_selection_index = 0;
            self.edit_selection_hovered_id = None;
        }
    }

    /// Update the hovered message ID based on current selection
    fn update_hovered_id(&mut self) {
        self.edit_selection_hovered_id = self.get_selected_message().map(|(id, _)| id.clone());
    }

    /// Get the current hovered message ID
    pub fn get_hovered_id(&self) -> Option<&str> {
        self.edit_selection_hovered_id.as_deref()
    }

    /// Clear edit selection state
    pub fn clear_edit_selection(&mut self) {
        self.edit_selection_messages.clear();
        self.edit_selection_index = 0;
        self.edit_selection_hovered_id = None;
    }

    /// Activate fuzzy finder
    pub fn activate_fuzzy(&mut self) {
        // The @ is one character before the cursor (since we just typed it)
        let cursor_pos = self.get_cursor_byte_offset();
        if cursor_pos > 0 {
            // The trigger is the @ just before the cursor
            self.fuzzy_finder.activate(cursor_pos - 1);
        } else {
            // Shouldn't happen, but handle gracefully
            self.fuzzy_finder.activate(0);
        }
    }

    /// Deactivate fuzzy finder
    pub fn deactivate_fuzzy(&mut self) {
        self.fuzzy_finder.deactivate();
    }

    /// Check if fuzzy finder is active
    pub fn fuzzy_active(&self) -> bool {
        self.fuzzy_finder.is_active()
    }

    /// Handle key event for fuzzy finder
    pub async fn handle_fuzzy_key(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
    ) -> Option<crate::tui::widgets::fuzzy_finder::FuzzyFinderResult> {
        use ratatui::crossterm::event::KeyCode;
        use tui_textarea::Input;

        // First handle navigation/selection in the fuzzy finder itself
        let result = self.fuzzy_finder.handle_input(key);

        if result.is_some() {
            // Key was handled (e.g., selection, closing), so just return the result
            return result;
        }

        // Block up/down arrows from reaching the textarea when fuzzy finder is active
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                // These keys are for fuzzy finder navigation only
                return None;
            }
            _ => {}
        }

        // Key was not for navigation, so treat it as text input
        let input = Input::from(key);
        self.textarea.input(input);

        // After input, check if we are still in a valid query.
        // If so, update results. If not, the finder should close.
        if let Some(query) = self.get_current_fuzzy_query() {
            let results = self.file_cache.fuzzy_search(&query, Some(10)).await;
            self.fuzzy_finder.update_results(results);
            None // Not a final result, just an update
        } else {
            // No longer a valid query, signal to close.
            Some(crate::tui::widgets::fuzzy_finder::FuzzyFinderResult::Close)
        }
    }

    /// Get file cache reference
    pub fn file_cache(&self) -> &FileCache {
        &self.file_cache
    }

    /// Get mutable file cache reference
    pub fn file_cache_mut(&mut self) -> &mut FileCache {
        &mut self.file_cache
    }
}

/// Properties for the [`InputPanel`] widget.
#[derive(Clone, Copy, Debug)]
pub struct InputPanel<'a> {
    pub input_mode: InputMode,
    pub current_approval: Option<&'a ToolCall>,
    pub is_processing: bool,
    pub spinner_state: usize,
}

impl<'a> InputPanel<'a> {
    pub fn new(
        input_mode: InputMode,
        current_approval: Option<&'a ToolCall>,
        is_processing: bool,
        spinner_state: usize,
    ) -> Self {
        Self {
            input_mode,
            current_approval,
            is_processing,
            spinner_state,
        }
    }
}

impl StatefulWidget for InputPanel<'_> {
    type State = InputPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Render approval prompt if needed
        if let Some(tool_call) = self.current_approval {
            let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
            let preview_lines = formatter.compact(
                &tool_call.parameters,
                &None,
                (area.width.saturating_sub(4)) as usize,
            );

            let mut approval_text = vec![
                Line::from(vec![
                    Span::styled("Tool ", Style::default().fg(Color::White)),
                    Span::styled(
                        &tool_call.name,
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" requests approval:", Style::default().fg(Color::White)),
                ]),
                Line::from(""),
            ];
            approval_text.extend(preview_lines);

            let title = Line::from(vec![
                Span::raw(" Tool Approval Required "),
                Span::raw("─ "),
                Span::styled(
                    "[Y]",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" once "),
                Span::styled(
                    "[A]",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("lways "),
                Span::styled(
                    "[N]",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::raw("o "),
            ]);

            let approval_block = Paragraph::new(approval_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .style(Style::default().fg(Color::Yellow)),
            );

            approval_block.render(area, buf);
            return;
        }

        // Normal input / edit selection rendering
        let input_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(
                "{}{}",
                if self.is_processing {
                    format!(" {}", get_spinner_char(self.spinner_state))
                } else {
                    String::new()
                },
                match self.input_mode {
                    InputMode::Insert => " Insert (Alt-Enter to send, Esc to cancel) ",
                    InputMode::Normal =>
                        " i to insert, ! for bash, u/d/j/k to scroll, e to edit previous messages ",
                    InputMode::BashCommand => " Bash (Enter to execute, Esc to cancel) ",
                    InputMode::AwaitingApproval => " Awaiting Approval ",
                    InputMode::SelectingModel => " Model Selection ",
                    InputMode::ConfirmExit =>
                        " Really quit? (y/Y to confirm, any other key to cancel) ",
                    InputMode::EditMessageSelection =>
                        " Select message to edit (↑↓ to navigate, Enter to select, Esc to cancel) ",
                    InputMode::FuzzyFinder => " ↑↓ to navigate, Enter to select, Esc to cancel ",
                }
            ))
            .style(match self.input_mode {
                InputMode::Insert => Style::default().fg(Color::Gray),
                InputMode::Normal => Style::default().fg(Color::DarkGray),
                InputMode::BashCommand => Style::default().fg(Color::Cyan),
                InputMode::ConfirmExit => Style::default().fg(Color::Red),
                InputMode::EditMessageSelection => Style::default().fg(Color::Yellow),
                InputMode::FuzzyFinder => Style::default().fg(Color::Gray),
                _ => Style::default(),
            });

        if self.input_mode == InputMode::EditMessageSelection {
            // Selection list rendering
            let mut items: Vec<ListItem> = Vec::new();
            if state.edit_selection_messages.is_empty() {
                items.push(
                    ListItem::new("No user messages to edit")
                        .style(Style::default().fg(Color::DarkGray)),
                );
            } else {
                let max_visible = 3;
                let total = state.edit_selection_messages.len();
                let (start_idx, end_idx) = if total <= max_visible {
                    (0, total)
                } else {
                    let half_window = max_visible / 2;
                    if state.edit_selection_index < half_window {
                        (0, max_visible)
                    } else if state.edit_selection_index >= total - half_window {
                        (total - max_visible, total)
                    } else {
                        let start = state.edit_selection_index - half_window;
                        (start, start + max_visible)
                    }
                };

                for idx in start_idx..end_idx {
                    let (_, content) = &state.edit_selection_messages[idx];
                    let preview = content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(area.width.saturating_sub(4) as usize)
                        .collect::<String>();
                    items.push(ListItem::new(preview));
                }

                let mut list_state = ListState::default();
                list_state.select(Some(state.edit_selection_index.saturating_sub(start_idx)));

                let highlight_style = Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::REVERSED);

                let list = List::new(items)
                    .block(input_block)
                    .highlight_style(highlight_style);
                StatefulWidget::render(list, area, buf, &mut list_state);
                return;
            }

            // Empty list fallback
            let list = List::new(items).block(input_block);
            Widget::render(list, area, buf);
            return;
        }

        // Default: textarea
        state.textarea.set_block(input_block);
        state.textarea.render(area, buf);

        // Scrollbar when needed
        let textarea_height = area.height.saturating_sub(2);
        let content_lines = state.textarea.lines().len();
        if content_lines > textarea_height as usize {
            let (cursor_row, _) = state.textarea.cursor();
            let mut scrollbar_state = ScrollbarState::new(content_lines)
                .position(cursor_row)
                .viewport_content_length(textarea_height as usize);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .thumb_style(Style::default().fg(Color::Gray));
            let scrollbar_area = Rect {
                x: area.x + area.width - 1,
                y: area.y + 1,
                width: 1,
                height: area.height - 2,
            };
            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::{ChatItem, MessageRow};

    #[test]
    fn test_input_panel_state_default() {
        let state = InputPanelState::default();
        assert!(state.edit_selection_messages.is_empty());
        assert_eq!(state.edit_selection_index, 0);
        assert!(state.edit_selection_hovered_id.is_none());
        assert_eq!(state.content(), "");
    }

    #[test]
    fn test_input_panel_state_content_operations() {
        let mut state = InputPanelState::default();

        // Test inserting text
        state.insert_str("Hello, world!");
        assert_eq!(state.content(), "Hello, world!");

        // Test clearing
        state.clear();
        assert_eq!(state.content(), "");

        // Test setting content from lines
        state.set_content_from_lines(vec!["Line 1", "Line 2", "Line 3"]);
        assert_eq!(state.content(), "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_edit_selection_navigation() {
        let mut state = InputPanelState {
            edit_selection_messages: vec![
                ("msg1".to_string(), "First message".to_string()),
                ("msg2".to_string(), "Second message".to_string()),
                ("msg3".to_string(), "Third message".to_string()),
            ],
            ..Default::default()
        };
        state.edit_selection_index = 1;
        state.update_hovered_id();

        // Test initial state
        assert_eq!(state.get_hovered_id(), Some("msg2"));

        // Test navigation up
        state.edit_selection_prev();
        assert_eq!(state.edit_selection_index, 0);
        assert_eq!(state.get_hovered_id(), Some("msg1"));

        // Test navigation at boundary
        state.edit_selection_prev();
        assert_eq!(state.edit_selection_index, 0);
        assert_eq!(state.get_hovered_id(), Some("msg1"));

        // Test navigation down
        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 1);
        assert_eq!(state.get_hovered_id(), Some("msg2"));

        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 2);
        assert_eq!(state.get_hovered_id(), Some("msg3"));

        // Test navigation at bottom boundary
        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 2);
        assert_eq!(state.get_hovered_id(), Some("msg3"));
    }

    #[test]
    fn test_clear_edit_selection() {
        let mut state = InputPanelState {
            edit_selection_messages: vec![("msg1".to_string(), "First message".to_string())],
            edit_selection_index: 0,
            edit_selection_hovered_id: Some("msg1".to_string()),
            ..Default::default()
        };

        // Clear it
        state.clear_edit_selection();

        assert!(state.edit_selection_messages.is_empty());
        assert_eq!(state.edit_selection_index, 0);
        assert!(state.edit_selection_hovered_id.is_none());
    }

    #[test]
    fn test_required_height_calculation() {
        let mut state = InputPanelState::default();

        // Empty textarea
        assert_eq!(state.required_height(10), 4); // 1 line + 3 for borders/padding

        // Multi-line content
        state.set_content_from_lines(vec!["Line 1", "Line 2", "Line 3"]);
        assert_eq!(state.required_height(10), 6); // 3 lines + 3

        // Test max height constraint
        state.set_content_from_lines(vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]);
        assert_eq!(state.required_height(8), 8); // Capped at max
    }

    #[test]
    fn test_populate_edit_selection() {
        let mut state = InputPanelState::default();

        // Create test chat items
        let chat_items = vec![
            ChatItem::Message(MessageRow::new(Message::User {
                id: "user1".to_string(),
                content: vec![UserContent::Text {
                    text: "First user message".to_string(),
                }],
                timestamp: 123,
                thread_id: uuid::Uuid::new_v4(),
                parent_message_id: None,
            })),
            ChatItem::Message(MessageRow::new(Message::Assistant {
                id: "assistant1".to_string(),
                content: vec![],
                timestamp: 124,
                thread_id: uuid::Uuid::new_v4(),
                parent_message_id: None,
            })),
            ChatItem::Message(MessageRow::new(Message::User {
                id: "user2".to_string(),
                content: vec![UserContent::Text {
                    text: "Second user message".to_string(),
                }],
                timestamp: 125,
                thread_id: uuid::Uuid::new_v4(),
                parent_message_id: None,
            })),
        ];

        state.populate_edit_selection(&chat_items);

        // Should have 2 user messages
        assert_eq!(state.edit_selection_messages.len(), 2);
        assert_eq!(state.edit_selection_messages[0].0, "user1");
        assert_eq!(state.edit_selection_messages[0].1, "First user message");
        assert_eq!(state.edit_selection_messages[1].0, "user2");
        assert_eq!(state.edit_selection_messages[1].1, "Second user message");

        // Should select the last message
        assert_eq!(state.edit_selection_index, 1);
        assert_eq!(state.get_hovered_id(), Some("user2"));
    }
}
