pub mod approval;
pub mod bash;
pub mod confirm_exit;
pub mod edit_selection;
pub mod fuzzy_finder;
pub mod setup;
pub mod simple;
pub mod text_manipulation;
pub mod vim;

mod setup_impl;

use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::KeyEvent;

impl Tui {
    pub async fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool> {
        // Check editing mode to determine handler
        match self.preferences.ui.editing_mode {
            steer_core::preferences::EditingMode::Simple => self.handle_simple_mode(key).await,
            steer_core::preferences::EditingMode::Vim => self.handle_vim_mode(key).await,
        }
    }
}
