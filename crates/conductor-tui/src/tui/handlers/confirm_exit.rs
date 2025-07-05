use crate::error::Result;
use crate::tui::{InputMode, Tui};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Tui {
    pub async fn handle_confirm_exit_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // User confirmed exit
                return Ok(true);
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+C again also confirms exit
                return Ok(true);
            }
            _ => {
                // Any other key cancels exit and returns to normal mode
                self.input_mode = InputMode::Normal;
            }
        }
        Ok(false)
    }
}
