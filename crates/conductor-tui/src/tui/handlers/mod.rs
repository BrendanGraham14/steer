pub mod approval;
pub mod bash;
pub mod confirm_exit;
pub mod edit_selection;
pub mod fuzzy_finder;
pub mod insert;
pub mod model_selection;
pub mod normal;

use crate::tui::{InputMode, Tui};
use anyhow::Result;
use ratatui::crossterm::event::KeyEvent;

impl Tui {
    pub async fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool> {
        match self.input_mode {
            InputMode::Normal => self.handle_normal_mode(key).await,
            InputMode::Insert => self.handle_insert_mode(key).await,
            InputMode::BashCommand => self.handle_bash_mode(key).await,
            InputMode::AwaitingApproval => self.handle_approval_mode(key).await,
            InputMode::SelectingModel => self.handle_model_selection_mode(key).await,
            InputMode::ConfirmExit => self.handle_confirm_exit_mode(key).await,
            InputMode::EditMessageSelection => self.handle_edit_selection_mode(key).await,
            InputMode::FuzzyFinder => self.handle_fuzzy_finder_mode(key).await,
        }
    }
}
