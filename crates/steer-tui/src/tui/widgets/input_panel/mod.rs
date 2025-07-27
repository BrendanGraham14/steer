//! Input panel widget module
//!
//! This module contains the input panel widget and its sub-components.

mod approval_prompt;
mod edit_selection;
mod fuzzy_state;
mod mode_title;
mod textarea;

pub use approval_prompt::ApprovalWidget;
pub use edit_selection::{EditSelectionState, EditSelectionWidget};
pub use fuzzy_state::FuzzyFinderHelper;
pub use mode_title::ModeTitleWidget;
pub use textarea::TextAreaWidget;

// Main input panel implementation
use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, StatefulWidget, Widget};
use ratatui::widgets::{Block, Borders};
use tui_textarea::{Input, TextArea};

use steer_tools::schema::ToolCall;

use crate::tui::InputMode;
use crate::tui::model::ChatItem;
use crate::tui::state::file_cache::FileCache;
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::fuzzy_finder::{FuzzyFinder, FuzzyFinderMode};

/// Stateful data for the [`InputPanel`] widget.
#[derive(Debug)]
pub struct InputPanelState {
    pub textarea: TextArea<'static>,
    pub edit_selection: EditSelectionState,
    pub file_cache: FileCache,
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
        textarea.set_cursor_line_style(ratatui::style::Style::default());
        textarea.set_cursor_style(
            ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::REVERSED),
        );
        Self {
            textarea,
            edit_selection: EditSelectionState::default(),
            file_cache: FileCache::new(session_id),
            fuzzy_finder: FuzzyFinder::new(),
        }
    }

    /// Get the content of the textarea
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Get the byte offset of the cursor in the textarea content
    pub fn get_cursor_byte_offset(&self) -> usize {
        let (row, col) = self.textarea.cursor();
        FuzzyFinderHelper::get_cursor_byte_offset(&self.content(), row, col)
    }

    /// Check if the fuzzy finder is active and the cursor is in a valid query position
    pub fn is_in_fuzzy_query(&self) -> bool {
        FuzzyFinderHelper::is_in_fuzzy_query(
            self.fuzzy_finder.trigger_position(),
            self.get_cursor_byte_offset(),
            &self.content(),
        )
    }

    /// Get the current fuzzy query if in a valid position
    pub fn get_current_fuzzy_query(&self) -> Option<String> {
        if self.is_in_fuzzy_query() {
            let trigger_pos = self.fuzzy_finder.trigger_position()?;
            FuzzyFinderHelper::get_current_fuzzy_query(
                trigger_pos,
                self.get_cursor_byte_offset(),
                &self.content(),
            )
        } else {
            None
        }
    }

    /// Complete fuzzy finder by replacing the query text with the selected item's insert text
    pub fn complete_picker_item(&mut self, item: &crate::tui::widgets::fuzzy_finder::PickerItem) {
        if let Some(trigger_pos) = self.fuzzy_finder.trigger_position() {
            let cursor_offset = self.get_cursor_byte_offset();
            let content = self.content();

            // Replace from the beginning of the trigger to the cursor position
            // Preserve text up to the trigger character
            let before_trigger = &content[..trigger_pos];
            let after_cursor = &content[cursor_offset..];

            let new_content = format!("{}{}{}", before_trigger, item.insert, after_cursor);

            // Calculate new cursor position (after the inserted text)
            let new_cursor_byte_pos = before_trigger.len() + item.insert.len();
            let new_cursor_row = new_content[..new_cursor_byte_pos].matches('\n').count();
            let last_newline_pos = new_content[..new_cursor_byte_pos]
                .rfind('\n')
                .map(|pos| pos + 1)
                .unwrap_or(0);
            let new_cursor_col = new_content[last_newline_pos..new_cursor_byte_pos]
                .chars()
                .count();

            self.textarea = TextArea::from(new_content.lines().collect::<Vec<_>>());
            self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
                new_cursor_row as u16,
                new_cursor_col as u16,
            ));
            self.fuzzy_finder.deactivate();
        }
    }

    /// Move to previous message in edit selection
    pub fn edit_selection_prev(&mut self) -> Option<&(String, String)> {
        self.edit_selection.select_prev()
    }

    /// Move to next message in edit selection
    pub fn edit_selection_next(&mut self) -> Option<&(String, String)> {
        self.edit_selection.select_next()
    }

    /// Get the currently selected message
    pub fn get_selected_message(&self) -> Option<&(String, String)> {
        self.edit_selection.get_selected()
    }

    /// Populate edit selection with messages from chat items
    pub fn populate_edit_selection<'a>(&mut self, chat_items: impl Iterator<Item = &'a ChatItem>) {
        self.edit_selection.populate_from_chat_items(chat_items);
    }

    /// Get the hovered edit selection ID
    pub fn get_hovered_edit_id(&self) -> Option<&str> {
        self.edit_selection.get_hovered_id()
    }

    /// Get the hovered edit selection ID (alias for compatibility)
    pub fn get_hovered_id(&self) -> Option<&str> {
        self.get_hovered_edit_id()
    }

    /// Clear edit selection
    pub fn clear_edit_selection(&mut self) {
        self.edit_selection.clear();
    }

    /// Activate fuzzy finder for files
    pub fn activate_fuzzy(&mut self) {
        let cursor_pos = self.get_cursor_byte_offset();
        let content = self.content();
        if cursor_pos > 0 && content.get(cursor_pos - 1..cursor_pos) == Some("@") {
            // The trigger position is the '@' character just before the cursor
            self.fuzzy_finder
                .activate(cursor_pos - 1, FuzzyFinderMode::Files);
        } else {
            self.fuzzy_finder.activate(0, FuzzyFinderMode::Files);
        }
    }

    /// Activate fuzzy finder for commands
    pub fn activate_command_fuzzy(&mut self) {
        let cursor_pos = self.get_cursor_byte_offset();
        let content = self.content();
        if content.get(cursor_pos..cursor_pos + 1) == Some("/") {
            self.fuzzy_finder
                .activate(cursor_pos + 1, FuzzyFinderMode::Commands);
        } else {
            self.fuzzy_finder.activate(0, FuzzyFinderMode::Commands);
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
        use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode;

        // First handle navigation/selection in the fuzzy finder itself
        let result = self.fuzzy_finder.handle_input(key);
        if result.is_some() {
            return result;
        }

        // Block up/down arrows from reaching the textarea when fuzzy finder is active
        use ratatui::crossterm::event::KeyCode;
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                // These keys are for fuzzy finder navigation only
                return None;
            }
            _ => {}
        }

        // Pass through to textarea for typing
        self.textarea.input(Input::from(key));

        // For escape and non-printable keys, check if we should close
        match self.content().chars().last() {
            Some(ch)
                if !ch.is_alphanumeric() && ch != '/' && ch != '.' && ch != '_' && ch != '-' =>
            {
                return Some(crate::tui::widgets::fuzzy_finder::FuzzyFinderResult::Close);
            }
            _ => {}
        }

        // After input, handle result updates based on the active fuzzy finder mode
        if self.fuzzy_finder.mode() == FuzzyFinderMode::Files {
            // Update fuzzy finder results based on current query
            if let Some(query) = self.get_current_fuzzy_query() {
                let file_results = self.file_cache.fuzzy_search(&query, Some(10)).await;
                // Convert file paths to PickerItems
                let picker_items = file_results
                    .into_iter()
                    .map(|path| {
                        crate::tui::widgets::fuzzy_finder::PickerItem::new(
                            path.clone(),
                            format!("@{path} "),
                        )
                    })
                    .collect();
                self.fuzzy_finder.update_results(picker_items);
                None
            } else {
                // No valid query, close the fuzzy finder
                Some(crate::tui::widgets::fuzzy_finder::FuzzyFinderResult::Close)
            }
        } else {
            None
        }
    }

    /// Clear the input
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Type your message here...");
        self.textarea
            .set_cursor_line_style(ratatui::style::Style::default());
        self.textarea.set_cursor_style(
            ratatui::style::Style::default().add_modifier(ratatui::style::Modifier::REVERSED),
        );
    }

    /// Replace the content and optionally set cursor position
    pub fn replace_content(&mut self, content: &str, cursor_pos: Option<(u16, u16)>) {
        self.textarea = TextArea::from(content.lines().collect::<Vec<_>>());
        if let Some((row, col)) = cursor_pos {
            self.textarea
                .move_cursor(tui_textarea::CursorMove::Jump(row, col));
        }
    }

    /// Check if there is content in the textarea
    pub fn has_content(&self) -> bool {
        !self.textarea.lines().is_empty() && !self.content().trim().is_empty()
    }

    /// Insert string at current cursor position
    pub fn insert_str(&mut self, text: &str) {
        self.textarea.insert_str(text);
    }

    /// Handle input event (passthrough to textarea)
    pub fn handle_input(&mut self, input: Input) {
        self.textarea.input(input);
    }

    /// Set content from lines
    pub fn set_content_from_lines(&mut self, lines: Vec<&str>) {
        self.textarea = TextArea::from(lines.into_iter().map(String::from).collect::<Vec<_>>());
    }

    /// Get file cache reference (compatibility method)
    pub fn file_cache(&self) -> &FileCache {
        &self.file_cache
    }

    /// Calculate required height for the input panel
    pub fn required_height(
        &self,
        current_approval: Option<&ToolCall>,
        width: u16,
        max_height: u16,
    ) -> u16 {
        if let Some(tool_call) = current_approval {
            // If there's a pending approval, use the approval height calculation
            Self::required_height_for_approval(tool_call, width, max_height)
        } else {
            // Otherwise use the regular calculation based on textarea lines
            let line_count = self.textarea.lines().len().max(1);
            // line count + 2 for borders + 1 for padding
            (line_count + 3).min(max_height as usize) as u16
        }
    }

    /// Calculate required height for approval mode
    pub fn required_height_for_approval(tool_call: &ToolCall, width: u16, max_height: u16) -> u16 {
        let theme = &Theme::default();
        let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
        let preview_lines = formatter.approval(
            &tool_call.parameters,
            width.saturating_sub(4) as usize,
            theme,
        );
        // 2 lines for header + preview lines + 2 for borders + 1 for padding
        (2 + preview_lines.len() + 3).min(max_height as usize) as u16
    }
}

/// Properties for the [`InputPanel`] widget.
#[derive(Clone, Copy, Debug)]
pub struct InputPanel<'a> {
    pub input_mode: InputMode,
    pub current_approval: Option<&'a ToolCall>,
    pub is_processing: bool,
    pub spinner_state: usize,
    pub theme: &'a Theme,
}

impl<'a> InputPanel<'a> {
    pub fn new(
        input_mode: InputMode,
        current_approval: Option<&'a ToolCall>,
        is_processing: bool,
        spinner_state: usize,
        theme: &'a Theme,
    ) -> Self {
        Self {
            input_mode,
            current_approval,
            is_processing,
            spinner_state,
            theme,
        }
    }
}

impl StatefulWidget for InputPanel<'_> {
    type State = InputPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Handle approval prompt
        if let Some(tool_call) = self.current_approval {
            ApprovalWidget::new(tool_call, self.theme).render(area, buf);
            return;
        }

        // Handle edit message selection mode
        if self.input_mode == InputMode::EditMessageSelection {
            let title = ModeTitleWidget::new(
                self.input_mode,
                self.is_processing,
                self.spinner_state,
                self.theme,
                state.has_content(),
            )
            .render();

            let block = Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(self.theme.style(Component::InputPanelBorderCommand))
                .border_style(self.theme.style(Component::InputPanelBorderCommand));

            EditSelectionWidget::new(self.theme).block(block).render(
                area,
                buf,
                &mut state.edit_selection,
            );
            return;
        }

        // Handle normal text input modes
        let title = ModeTitleWidget::new(
            self.input_mode,
            self.is_processing,
            self.spinner_state,
            self.theme,
            state.has_content(),
        )
        .render();

        let block = Block::default().borders(Borders::ALL).title(title);

        TextAreaWidget::new(&mut state.textarea, self.theme)
            .with_block(block)
            .with_mode(self.input_mode)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_panel_state_default() {
        let state = InputPanelState::default();
        assert!(state.edit_selection.messages.is_empty());
        assert_eq!(state.edit_selection.selected_index, 0);
        assert!(state.edit_selection.hovered_id.is_none());
        assert_eq!(state.content(), "");
    }

    #[test]
    fn test_content_manipulation() {
        let mut state = InputPanelState::default();

        // Test setting content
        state.replace_content("Hello\nWorld", None);
        assert_eq!(state.content(), "Hello\nWorld");

        // Test clearing
        state.clear();
        assert_eq!(state.content(), "");
        assert!(!state.has_content());
    }

    #[test]
    fn test_fuzzy_finder_activation() {
        let mut state = InputPanelState::default();

        // Set up content with @ trigger
        state.replace_content("Check @", Some((0, 7)));

        // Activate fuzzy finder
        state.activate_fuzzy();
        assert!(state.fuzzy_active());

        // Deactivate
        state.deactivate_fuzzy();
        assert!(!state.fuzzy_active());
    }

    #[test]
    fn test_cursor_byte_offset() {
        let mut state = InputPanelState::default();
        state.replace_content("Hello\nWorld", Some((1, 3)));

        // Cursor at "Wor|ld" (row 1, col 3)
        assert_eq!(state.get_cursor_byte_offset(), 9); // "Hello\n" (6) + "Wor" (3)
    }
}
