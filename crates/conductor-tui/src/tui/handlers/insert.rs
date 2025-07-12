use crate::error::Result;
use crate::tui::{InputMode, Tui};
use conductor_core::app::AppCommand;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tracing::{debug, info};
use tui_textarea::{CursorMove, Input};

impl Tui {
    pub async fn handle_insert_mode(&mut self, key: KeyEvent) -> Result<bool> {
        let input = Input::from(key);
        debug!(
            target: "tui.insert",
            "Insert mode key event - code: {:?}, modifiers: {:?}",
            key.code, key.modifiers
        );

        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.is_processing {
                    // Cancel processing
                    self.command_sink
                        .send_command(AppCommand::CancelProcessing)
                        .await?;
                } else {
                    // Go to exit confirmation mode
                    self.input_mode = InputMode::ConfirmExit;
                }
                Ok(false)
            }
            (KeyCode::Enter, KeyModifiers::NONE) => {
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
                Ok(false)
            }
            (KeyCode::Enter, KeyModifiers::SHIFT)
            | (KeyCode::Enter, KeyModifiers::ALT)
            | (KeyCode::Enter, KeyModifiers::CONTROL)
            | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.input_panel_state
                    .handle_input(Input::from(KeyEvent::new(
                        KeyCode::Char('\n'),
                        KeyModifiers::empty(),
                    )));
                Ok(false)
            }
            // Character
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Back);
                Ok(false)
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Forward);
                Ok(false)
            }
            // Word
            (KeyCode::Left, KeyModifiers::ALT) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::WordBack);
                Ok(false)
            }
            (KeyCode::Right, KeyModifiers::ALT) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::WordForward);
                Ok(false)
            }
            // Line
            (KeyCode::Left, KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Head);
                Ok(false)
            }
            (KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SUPER) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                Ok(false)
            }

            (KeyCode::Up, KeyModifiers::NONE) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::Up);
                Ok(false)
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Down);
                Ok(false)
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_head();
                Ok(false)
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_end();
                Ok(false)
            }
            (KeyCode::Backspace, KeyModifiers::SUPER | KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_head();
                Ok(false)
            }
            (KeyCode::Delete, KeyModifiers::SUPER | KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_end();
                Ok(false)
            }
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                // Check if we're at the start of an empty input
                let content = self.input_panel_state.content();
                let cursor_pos = self.input_panel_state.get_cursor_byte_offset();

                if content.is_empty() && cursor_pos == 0 {
                    // First, insert the / character
                    self.input_panel_state.handle_input(input);

                    // Activate fuzzy finder for commands
                    self.input_panel_state.activate_command_fuzzy();
                    self.input_mode = InputMode::FuzzyFinder;

                    info!(target: "tui.fuzzy_finder", "Activated command fuzzy finder");

                    // Perform initial search with all commands
                    let results: Vec<String> = self
                        .command_registry
                        .all_commands()
                        .into_iter()
                        .map(|cmd| cmd.name.to_string())
                        .collect();
                    self.input_panel_state.fuzzy_finder.update_results(results);

                    Ok(false)
                } else {
                    // Normal / character insertion
                    self.input_panel_state.handle_input(input);
                    Ok(false)
                }
            }
            (KeyCode::Char('@'), KeyModifiers::NONE) => {
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
                Ok(false)
            }
            (KeyCode::Esc, KeyModifiers::NONE) => {
                // Return to normal mode without clearing text
                self.input_mode = InputMode::Normal;
                Ok(false)
            }
            _ => {
                // Let input panel state handle the input
                self.input_panel_state.handle_input(input);
                Ok(false)
            }
        }
    }
}
