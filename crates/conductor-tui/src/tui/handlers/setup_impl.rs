use super::setup::SetupHandler;
use crate::error::Result;
use crate::tui::{InputMode, Tui};
use ratatui::crossterm::event::KeyEvent;

impl Tui {
    pub async fn handle_setup_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some(new_mode) = SetupHandler::handle_key_event(self, key).await? {
            self.input_mode = new_mode;

            if new_mode == InputMode::Normal {
                self.setup_state = None;
            }
        }
        Ok(false)
    }
}
