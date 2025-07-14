use super::setup::SetupHandler;
use crate::error::Result;
use crate::tui::InputMode;
use crate::tui::Tui;
use ratatui::crossterm::event::KeyEvent;

impl Tui {
    pub async fn handle_setup_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some(new_mode) = SetupHandler::handle_key_event(self, key).await? {
            // Check if setup is complete by looking for the default modes
            if new_mode == InputMode::Simple || new_mode == InputMode::VimNormal {
                self.input_mode = self.default_input_mode();
                self.setup_state = None;
            } else {
                self.input_mode = new_mode;
            }
        }
        Ok(false)
    }
}
