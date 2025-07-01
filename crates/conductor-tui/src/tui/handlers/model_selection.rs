use crate::tui::{InputMode, Tui};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

impl Tui {
    pub async fn handle_model_selection_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Enter => {
                if let Some(selected) = self.popup_state.selected() {
                    if selected < self.models.len() {
                        let new_model = self.models[selected];
                        self.current_model = new_model;
                        // Send model change as a slash command
                        let command = format!("/model {}", new_model);
                        self.handle_slash_command(command).await?;
                    }
                }
                self.input_mode = InputMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.popup_state.previous();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.popup_state.next(self.models.len());
            }
            _ => {}
        }
        Ok(false)
    }
}
