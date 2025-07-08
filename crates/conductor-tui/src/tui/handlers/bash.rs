use crate::error::Result;
use crate::tui::{InputMode, Tui};
use conductor_core::app::AppCommand;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::CursorMove;

impl Tui {
    pub async fn handle_bash_mode(&mut self, key: KeyEvent) -> Result<bool> {
        // Check for Ctrl+C
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.is_processing {
                // Cancel processing
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            } else {
                // Cancel bash mode and return to normal without clearing text
                self.input_mode = InputMode::Normal;
                self.input_panel_state
                    .textarea
                    .set_placeholder_text("Type your message here...");
            }
            return Ok(false);
        }

        // Check for Esc
        if key.code == KeyCode::Esc {
            // Return to normal mode without clearing text
            self.input_mode = InputMode::Normal;
            self.input_panel_state
                .textarea
                .set_placeholder_text("Type your message here...");
        // Check for Alt+Left/Right for word navigation
        } else if key.modifiers == KeyModifiers::ALT {
            match key.code {
                KeyCode::Left => {
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::WordBack);
                }
                KeyCode::Right => {
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::WordForward);
                }
                _ => {
                    // Convert KeyEvent to Input and let the panel state handle it
                    let input = tui_textarea::Input::from(key);
                    self.input_panel_state.handle_input(input);
                }
            }
        } else if key.code == KeyCode::Enter {
            // Execute the bash command
            let command = self.input_panel_state.content();
            if !command.trim().is_empty() {
                self.command_sink
                    .send_command(AppCommand::ExecuteBashCommand { command })
                    .await?;
                self.input_panel_state.clear(); // Clear after executing
                self.input_mode = InputMode::Normal;
                self.input_panel_state
                    .textarea
                    .set_placeholder_text("Type your message here...");
            }
        } else {
            // Convert KeyEvent to Input and let the panel state handle it
            let input = tui_textarea::Input::from(key);
            self.input_panel_state.handle_input(input);
        }
        Ok(false)
    }
}
