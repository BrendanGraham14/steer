use crate::error::Result;
use crate::tui::{InputMode, PopupState, Tui, ViewMode};
use conductor_core::app::AppCommand;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

impl Tui {
    pub async fn handle_normal_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Char('i') => {
                self.input_mode = InputMode::Insert;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.view_model.chat_list_state.scroll_down(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.view_model.chat_list_state.scroll_up(1);
            }
            KeyCode::Char('g') => {
                self.view_model.chat_list_state.scroll_to_top();
            }
            KeyCode::Char('G') => {
                self.view_model.chat_list_state.scroll_to_bottom();
            }
            KeyCode::Char('e') => {
                // Enter edit message selection mode
                self.enter_edit_selection_mode();
            }
            KeyCode::PageUp => {
                self.view_model.chat_list_state.scroll_up(10);
            }
            KeyCode::PageDown => {
                self.view_model.chat_list_state.scroll_down(10);
            }
            KeyCode::Char('d') => {
                let page_size = self.terminal_size.1.saturating_sub(6) / 2;
                self.view_model.chat_list_state.scroll_down(page_size);
            }
            KeyCode::Char('u') => {
                let page_size = self.terminal_size.1.saturating_sub(6) / 2;
                self.view_model.chat_list_state.scroll_up(page_size);
            }
            KeyCode::Home => {
                self.view_model.chat_list_state.scroll_to_top();
            }
            KeyCode::End => {
                self.view_model.chat_list_state.scroll_to_bottom();
            }
            KeyCode::Char('v') => {
                self.view_model.chat_list_state.view_mode =
                    match self.view_model.chat_list_state.view_mode {
                        ViewMode::Compact => ViewMode::Detailed,
                        ViewMode::Detailed => ViewMode::Compact,
                    };
            }
            KeyCode::Char('D') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.view_model.chat_list_state.view_mode = ViewMode::Detailed;
            }
            KeyCode::Char('C') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.view_model.chat_list_state.view_mode = ViewMode::Compact;
            }
            KeyCode::Esc => {
                // Cancel current processing if any
                self.command_sink
                    .send_command(AppCommand::CancelProcessing)
                    .await?;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.is_processing {
                    // Cancel processing
                    self.command_sink
                        .send_command(AppCommand::CancelProcessing)
                        .await?;
                } else {
                    // Enter exit confirmation mode
                    self.input_mode = InputMode::ConfirmExit;
                }
            }
            KeyCode::Char('!') => {
                // Enter bash command mode
                self.input_mode = InputMode::BashCommand;
                self.input_panel_state
                    .textarea
                    .set_placeholder_text("Type your bash command here...");
            }
            KeyCode::Char('m') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-M: Show model selection popup
                self.popup_state = PopupState::default();
                // Find current model index
                if let Some(index) = self.models.iter().position(|m| m == &self.current_model) {
                    self.popup_state.select(Some(index));
                }
            }
            _ => {}
        }
        Ok(false)
    }
}
