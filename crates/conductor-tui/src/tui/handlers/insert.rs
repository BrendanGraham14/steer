use crate::tui::{InputMode, Tui};
use anyhow::Result;
use conductor_core::app::AppCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tracing::info;
use tui_textarea::Input;

impl Tui {
    pub async fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<bool> {
        let input = Input::from(key);

        // Check for Ctrl+C
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.is_processing {
                // Cancel processing
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            } else {
                // Go to exit confirmation mode
                self.input_mode = InputMode::ConfirmExit;
            }
            return Ok(false);
        }

        // Check for Alt+Enter before passing to textarea
        if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::ALT {
            // Send message
            let content = self.input_panel_state.content();
            if !content.trim().is_empty() {
                self.send_message(content).await?;
                self.input_panel_state.clear(); // Clear after sending
                self.input_mode = InputMode::Normal;
            }
            return Ok(false);
        }

        // Check if we should trigger fuzzy finder
        if key.code == KeyCode::Char('@') && !self.input_panel_state.fuzzy_active() {
            // First, insert the @ character
            self.input_panel_state.handle_input(input);

            // Then activate fuzzy finder
            self.input_panel_state.activate_fuzzy();
            self.input_mode = InputMode::FuzzyFinder;

            // Log cache status
            let cache_size = self.input_panel_state.file_cache().len().await;
            info!(target: "tui.fuzzy_finder", "Activated fuzzy finder, cache has {} files", cache_size);

            // Perform initial search with all files
            let results = self
                .input_panel_state
                .file_cache()
                .fuzzy_search("", Some(20))
                .await;
            self.input_panel_state.fuzzy_finder.update_results(results);
            return Ok(false);
        }

        match input {
            Input {
                key: tui_textarea::Key::Esc,
                ..
            } => {
                // Return to normal mode without clearing text
                self.input_mode = InputMode::Normal;
            }
            _ => {
                // Let input panel state handle the input
                self.input_panel_state.handle_input(input);
            }
        }
        Ok(false)
    }
}
