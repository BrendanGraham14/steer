use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Tui {
    pub async fn handle_edit_selection_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                // Exit edit selection mode
                self.input_mode = self.default_input_mode();
                self.input_panel_state.clear_edit_selection();
            }
            (KeyCode::Enter, _) => {
                // Select the currently highlighted message
                if let Some((message_id, _)) = self.input_panel_state.get_selected_message() {
                    let message_id = message_id.clone();
                    self.enter_edit_mode(&message_id);
                    self.input_panel_state.clear_edit_selection();
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
                // Move selection up
                self.input_panel_state.edit_selection_prev();
                if let Some(id) = self.input_panel_state.get_hovered_id() {
                    let id = id.to_string();
                    self.scroll_to_message_id(&id);
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
                // Move selection down
                self.input_panel_state.edit_selection_next();
                if let Some(id) = self.input_panel_state.get_hovered_id() {
                    let id = id.to_string();
                    self.scroll_to_message_id(&id);
                }
            }
            _ => {}
        }
        Ok(false)
    }
}
