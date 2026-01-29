use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Tui {
    pub async fn handle_edit_selection_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c' | 'd'), KeyModifiers::CONTROL) => {
                self.input_mode = self.default_input_mode();
                self.edit_selection_state.clear();
            }
            (KeyCode::Enter, _) => {
                if let Some((message_id, _)) = self.edit_selection_state.get_selected().cloned() {
                    self.enter_edit_mode(&message_id);
                    self.edit_selection_state.clear();
                }
            }
            (KeyCode::Up | KeyCode::Char('k'), _) => {
                self.edit_selection_state.select_prev();
            }
            (KeyCode::Down | KeyCode::Char('j'), _) => {
                self.edit_selection_state.select_next();
            }
            _ => {}
        }
        Ok(false)
    }
}
