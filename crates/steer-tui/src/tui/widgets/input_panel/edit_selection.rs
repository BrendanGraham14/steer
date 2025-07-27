//! Edit selection widget for browsing and selecting previous messages

use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, StatefulWidget};
use ratatui::style::Modifier;
use ratatui::widgets::{Block, List, ListItem, ListState};

use steer_core::app::conversation::Role;

use crate::tui::model::{ChatItem, ChatItemData};
use crate::tui::theme::{Component, Theme};

/// State for the edit selection widget
#[derive(Debug, Default)]
pub struct EditSelectionState {
    pub messages: Vec<(String, String)>,
    pub selected_index: usize,
    pub hovered_id: Option<String>,
}

impl EditSelectionState {
    /// Move selection to previous message
    pub fn select_prev(&mut self) -> Option<&(String, String)> {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
        self.messages.get(self.selected_index)
    }

    /// Move selection to next message
    pub fn select_next(&mut self) -> Option<&(String, String)> {
        if self.selected_index + 1 < self.messages.len() {
            self.selected_index += 1;
        }
        self.messages.get(self.selected_index)
    }

    /// Get currently selected message
    pub fn get_selected(&self) -> Option<&(String, String)> {
        self.messages.get(self.selected_index)
    }

    /// Populate the selection with messages from chat items
    pub fn populate_from_chat_items<'a>(&mut self, chat_items: impl Iterator<Item = &'a ChatItem>) {
        self.messages = chat_items
            .filter_map(|item| match &item.data {
                ChatItemData::Message(message) => {
                    if message.role() == Role::User {
                        Some((item.id().to_string(), message.content_string()))
                    } else {
                        None
                    }
                }
                _ => None,
            })
            .collect();

        // Select the last message by default
        if !self.messages.is_empty() {
            self.selected_index = self.messages.len() - 1;
        } else {
            self.selected_index = 0;
            self.hovered_id = None;
            return;
        }

        // Update hovered ID
        self.hovered_id = self.get_selected().map(|(id, _)| id.clone());
    }

    /// Get the ID of the currently hovered message
    pub fn get_hovered_id(&self) -> Option<&str> {
        self.hovered_id.as_deref()
    }

    /// Clear all selection state
    pub fn clear(&mut self) {
        self.messages.clear();
        self.selected_index = 0;
        self.hovered_id = None;
    }
}

/// Widget for displaying and selecting previous messages
#[derive(Debug)]
pub struct EditSelectionWidget<'a> {
    theme: &'a Theme,
    block: Option<Block<'a>>,
}

impl<'a> EditSelectionWidget<'a> {
    /// Create a new edit selection widget
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme, block: None }
    }

    /// Set the block for the widget
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// Calculate the visible window for the selection list
    fn calculate_window(
        &self,
        total: usize,
        selected: usize,
        max_visible: usize,
    ) -> (usize, usize) {
        if total <= max_visible {
            (0, total)
        } else {
            let half_window = max_visible / 2;
            if selected < half_window {
                (0, max_visible)
            } else if selected >= total - half_window {
                (total - max_visible, total)
            } else {
                let start = selected - half_window;
                (start, start + max_visible)
            }
        }
    }

    /// Format a message preview for display
    fn format_message_preview(&self, content: &str, max_width: usize) -> String {
        content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(max_width)
            .collect()
    }
}

impl<'a> StatefulWidget for EditSelectionWidget<'a> {
    type State = EditSelectionState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let mut items: Vec<ListItem> = Vec::new();

        if state.messages.is_empty() {
            items.push(
                ListItem::new("No user messages to edit")
                    .style(self.theme.style(Component::DimText)),
            );

            // Render empty list
            let list = List::new(items);
            let list = if let Some(block) = self.block {
                list.block(block)
            } else {
                list
            };
            let mut list_state = ListState::default();
            StatefulWidget::render(list, area, buf, &mut list_state);
            return;
        }

        // Calculate visible window
        let max_visible = 3;
        let (start_idx, end_idx) =
            self.calculate_window(state.messages.len(), state.selected_index, max_visible);

        // Create list items for visible range
        let max_width = area.width.saturating_sub(4) as usize;
        for idx in start_idx..end_idx {
            let (_, content) = &state.messages[idx];
            let preview = self.format_message_preview(content, max_width);
            items.push(ListItem::new(preview));
        }

        // Set up list state
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected_index.saturating_sub(start_idx)));

        // Create and render list
        let highlight_style = self
            .theme
            .style(Component::SelectionHighlight)
            .add_modifier(Modifier::REVERSED);

        let list = List::new(items).highlight_style(highlight_style);
        let list = if let Some(block) = self.block {
            list.block(block)
        } else {
            list
        };

        StatefulWidget::render(list, area, buf, &mut list_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edit_selection_state_default() {
        let state = EditSelectionState::default();
        assert!(state.messages.is_empty());
        assert_eq!(state.selected_index, 0);
        assert!(state.hovered_id.is_none());
    }

    #[test]
    fn test_edit_selection_navigation() {
        let mut state = EditSelectionState {
            messages: vec![
                ("msg1".to_string(), "First message".to_string()),
                ("msg2".to_string(), "Second message".to_string()),
                ("msg3".to_string(), "Third message".to_string()),
            ],
            selected_index: 1,
            hovered_id: Some("msg2".to_string()),
        };

        // Test moving previous
        state.select_prev();
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.get_selected().unwrap().0, "msg1");

        // Test boundary - can't go before first
        state.select_prev();
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.get_selected().unwrap().0, "msg1");

        // Test moving next
        state.select_next();
        assert_eq!(state.selected_index, 1);
        assert_eq!(state.get_selected().unwrap().0, "msg2");

        state.select_next();
        assert_eq!(state.selected_index, 2);
        assert_eq!(state.get_selected().unwrap().0, "msg3");

        // Test boundary - can't go past last
        state.select_next();
        assert_eq!(state.selected_index, 2);
        assert_eq!(state.get_selected().unwrap().0, "msg3");
    }

    #[test]
    fn test_clear_selection() {
        let mut state = EditSelectionState {
            messages: vec![("msg1".to_string(), "First message".to_string())],
            selected_index: 0,
            hovered_id: Some("msg1".to_string()),
        };

        state.clear();
        assert!(state.messages.is_empty());
        assert_eq!(state.selected_index, 0);
        assert!(state.hovered_id.is_none());
    }

    #[test]
    fn test_populate_from_chat_items() {
        use steer_core::app::conversation::{AssistantContent, Message, MessageData, UserContent};

        let chat_items = vec![
            ChatItem {
                parent_chat_item_id: None,
                data: ChatItemData::Message(Message {
                    timestamp: 1000,
                    id: "user1".to_string(),
                    parent_message_id: None,
                    data: MessageData::User {
                        content: vec![UserContent::Text {
                            text: "First user message".to_string(),
                        }],
                    },
                }),
            },
            ChatItem {
                parent_chat_item_id: None,
                data: ChatItemData::Message(Message {
                    timestamp: 2000,
                    id: "assistant1".to_string(),
                    parent_message_id: Some("user1".to_string()),
                    data: MessageData::Assistant {
                        content: vec![AssistantContent::Text {
                            text: "Assistant response".to_string(),
                        }],
                    },
                }),
            },
            ChatItem {
                parent_chat_item_id: None,
                data: ChatItemData::Message(Message {
                    timestamp: 3000,
                    id: "user2".to_string(),
                    parent_message_id: Some("assistant1".to_string()),
                    data: MessageData::User {
                        content: vec![UserContent::Text {
                            text: "Second user message".to_string(),
                        }],
                    },
                }),
            },
        ];

        let mut state = EditSelectionState::default();
        state.populate_from_chat_items(chat_items.iter());

        // Should only include user messages
        assert_eq!(state.messages.len(), 2);
        assert_eq!(state.messages[0].0, "user1");
        assert_eq!(state.messages[0].1, "First user message");
        assert_eq!(state.messages[1].0, "user2");
        assert_eq!(state.messages[1].1, "Second user message");

        // Should select the last message
        assert_eq!(state.selected_index, 1);
        assert_eq!(state.hovered_id, Some("user2".to_string()));
    }
}
