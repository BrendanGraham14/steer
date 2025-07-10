use crate::error::Result;
use crate::tui::{InputMode, Tui};
use conductor_core::app::AppCommand;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tracing::info;
use tui_textarea::{CursorMove, Input};

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

        // Check for plain Enter to send message
        if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::empty() {
            // Send message
            let content = self.input_panel_state.content();
            if !content.trim().is_empty() {
                // Store the current mode before sending
                let mode_before = self.input_mode;
                self.send_message(content).await?;
                self.input_panel_state.clear(); // Clear after sending

                // Only return to Normal mode if we're still in Insert mode
                // (i.e., the command didn't change the mode)
                if self.input_mode == mode_before {
                    self.input_mode = InputMode::Normal;
                }
            }
            return Ok(false);
        }

        // Check for various newline key combinations
        if (key.code == KeyCode::Enter
            && (key.modifiers == KeyModifiers::SHIFT
                || key.modifiers == KeyModifiers::ALT
                || key.modifiers == KeyModifiers::CONTROL))
            || (key.code == KeyCode::Char('j') && key.modifiers == KeyModifiers::CONTROL)
        {
            self.input_panel_state
                .handle_input(Input::from(KeyEvent::new(
                    KeyCode::Char('\n'),
                    KeyModifiers::empty(),
                )));
            return Ok(false);
        }

        // Check for Alt+Left/Right for word navigation
        if key.modifiers == KeyModifiers::ALT {
            match key.code {
                KeyCode::Left => {
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::WordBack);
                    return Ok(false);
                }
                KeyCode::Right => {
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::WordForward);
                    return Ok(false);
                }
                _ => {}
            }
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
