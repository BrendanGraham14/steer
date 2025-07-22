use crate::error::Result;
use crate::tui::Tui;
use crate::tui::{InputMode, VimOperator};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use steer_core::app::AppCommand;
use tui_textarea::{CursorMove, Input};

impl Tui {
    pub async fn handle_vim_mode(&mut self, key: KeyEvent) -> Result<bool> {
        match self.input_mode {
            InputMode::VimNormal => self.handle_vim_normal(key).await,
            InputMode::VimInsert => self.handle_vim_insert(key).await,
            // Other special modes that override vim
            InputMode::BashCommand => self.handle_bash_mode(key).await,
            InputMode::AwaitingApproval => self.handle_approval_mode(key).await,
            InputMode::EditMessageSelection => self.handle_edit_selection_mode(key).await,
            InputMode::FuzzyFinder => self.handle_fuzzy_finder_mode(key).await,
            InputMode::ConfirmExit => self.handle_confirm_exit_mode(key).await,
            InputMode::Setup => self.handle_setup_mode(key).await,
            InputMode::Simple => self.handle_simple_mode(key).await, // Fallback
        }
    }

    async fn handle_vim_normal(&mut self, key: KeyEvent) -> Result<bool> {
        let mut should_clear_state = true;

        // Handle modified keys first
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if self.is_processing {
                        self.command_sink
                            .send_command(AppCommand::CancelProcessing)
                            .await?;
                    } else {
                        self.switch_mode(InputMode::ConfirmExit);
                    }
                }
                KeyCode::Char('r') => {
                    if self.vim_state.pending_operator.is_some() {
                        // Redo when operator pending
                        self.input_panel_state.textarea.redo();
                    } else {
                        // Toggle view mode otherwise
                        self.chat_viewport.state_mut().toggle_view_mode();
                    }
                }
                KeyCode::Char('u') => self.chat_viewport.state_mut().scroll_up(10),
                KeyCode::Char('d') => self.chat_viewport.state_mut().scroll_down(10),
                _ => {}
            }
            return Ok(false);
        }

        // Handle operator-pending mode
        if let Some(operator) = self.vim_state.pending_operator {
            let mut motion_handled = true;
            match key.code {
                // Motions
                KeyCode::Char('w') => {
                    if operator == VimOperator::Change {
                        self.input_panel_state.textarea.delete_next_word();
                        // Delete trailing whitespace
                        while let Some(line) = self
                            .input_panel_state
                            .textarea
                            .lines()
                            .get(self.input_panel_state.textarea.cursor().0)
                        {
                            if let Some(ch) =
                                line.chars().nth(self.input_panel_state.textarea.cursor().1)
                            {
                                if ch == ' ' || ch == '\t' {
                                    self.input_panel_state.textarea.delete_next_char();
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                        self.set_mode(InputMode::VimInsert);
                    } else if operator == VimOperator::Delete {
                        self.input_panel_state.textarea.delete_next_word();
                        // Delete trailing whitespace
                        while let Some(line) = self
                            .input_panel_state
                            .textarea
                            .lines()
                            .get(self.input_panel_state.textarea.cursor().0)
                        {
                            if let Some(ch) =
                                line.chars().nth(self.input_panel_state.textarea.cursor().1)
                            {
                                if ch == ' ' || ch == '\t' {
                                    self.input_panel_state.textarea.delete_next_char();
                                } else {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                }
                KeyCode::Char('b') => {
                    if operator == VimOperator::Change {
                        self.input_panel_state.textarea.delete_word();
                        self.set_mode(InputMode::VimInsert);
                    } else if operator == VimOperator::Delete {
                        self.input_panel_state.textarea.delete_word();
                    }
                }
                KeyCode::Char('$') => {
                    if operator == VimOperator::Change {
                        self.input_panel_state.textarea.delete_line_by_end();
                        self.set_mode(InputMode::VimInsert);
                    } else if operator == VimOperator::Delete {
                        self.input_panel_state.textarea.delete_line_by_end();
                    }
                }
                KeyCode::Char('0') | KeyCode::Char('^') => {
                    if operator == VimOperator::Change {
                        self.input_panel_state.textarea.delete_line_by_head();
                        self.set_mode(InputMode::VimInsert);
                    } else if operator == VimOperator::Delete {
                        self.input_panel_state.textarea.delete_line_by_head();
                    }
                }
                KeyCode::Esc => { /* Cancel operator */ }
                _ => {
                    motion_handled = false;
                }
            }

            if motion_handled {
                self.vim_state.pending_operator = None;
                return Ok(false);
            }
        }

        // Normal mode commands
        match key.code {
            // ESC handling: double-tap clears / edits, single tap cancels or records
            KeyCode::Esc => {
                // Detect double ESC within 300 ms
                if self
                    .double_tap_tracker
                    .is_double_tap(KeyCode::Esc, Duration::from_millis(300))
                {
                    if self.input_panel_state.content().is_empty() {
                        // Empty input => open edit-previous-message picker
                        self.enter_edit_selection_mode();
                    } else {
                        // Otherwise just clear the buffer
                        self.input_panel_state.clear();
                    }
                    // Clear tracker to avoid triple-tap behaviour
                    self.double_tap_tracker.clear_key(&KeyCode::Esc);

                    // Reset vim transient state
                    self.vim_state.pending_operator = None;
                    self.vim_state.pending_g = false;
                    self.vim_state.replace_mode = false;
                    return Ok(false);
                }

                // First Esc press – record and perform normal cancel behaviour
                self.double_tap_tracker.record_key(KeyCode::Esc);

                if self.vim_state.visual_mode {
                    self.vim_state.visual_mode = false;
                    // Cancel selection
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::Forward);
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::Back);
                } else if self.is_processing {
                    self.command_sink
                        .send_command(AppCommand::CancelProcessing)
                        .await?;
                }
                self.vim_state.pending_operator = None;
                self.vim_state.pending_g = false;
                self.vim_state.replace_mode = false;
            }
            // New operators
            KeyCode::Char('d') => {
                if self.vim_state.pending_operator == Some(VimOperator::Delete) {
                    // dd - delete line
                    self.input_panel_state.clear();
                    self.vim_state.pending_operator = None;
                } else {
                    self.vim_state.pending_operator = Some(VimOperator::Delete);
                    // Keep operator pending until motion key arrives
                    should_clear_state = false;
                }
            }
            KeyCode::Char('c') => {
                if self.vim_state.pending_operator == Some(VimOperator::Change) {
                    // cc - change line
                    self.input_panel_state.clear();
                    self.set_mode(InputMode::VimInsert);
                    self.vim_state.pending_operator = None;
                } else {
                    self.vim_state.pending_operator = Some(VimOperator::Change);
                    should_clear_state = false;
                }
            }
            KeyCode::Char('y') => {
                if self.vim_state.pending_operator == Some(VimOperator::Yank) {
                    // yy - yank line
                    self.input_panel_state.textarea.copy();
                    self.vim_state.pending_operator = None;
                } else {
                    self.vim_state.pending_operator = Some(VimOperator::Yank);
                    should_clear_state = false;
                }
            }

            // Mode changes
            KeyCode::Char('i') => self.set_mode(InputMode::VimInsert),
            KeyCode::Char('I') => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Head);
                self.set_mode(InputMode::VimInsert);
            }
            KeyCode::Char('a') => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Forward);
                self.set_mode(InputMode::VimInsert);
            }
            KeyCode::Char('A') => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                self.set_mode(InputMode::VimInsert);
            }
            KeyCode::Char('o') => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                self.input_panel_state.insert_str("\n");
                self.set_mode(InputMode::VimInsert);
            }
            KeyCode::Char('O') => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Head);
                self.input_panel_state.insert_str("\n");
                self.input_panel_state.textarea.move_cursor(CursorMove::Up);
                self.set_mode(InputMode::VimInsert);
            }

            // Text manipulation
            KeyCode::Char('x') => {
                self.input_panel_state.textarea.delete_next_char();
            }
            KeyCode::Char('X') => {
                self.input_panel_state.textarea.delete_char();
            }
            KeyCode::Char('D') => {
                self.input_panel_state.textarea.delete_line_by_end();
            }
            KeyCode::Char('C') => {
                self.input_panel_state.textarea.delete_line_by_end();
                self.set_mode(InputMode::VimInsert);
            }
            KeyCode::Char('p') => {
                self.input_panel_state.textarea.paste();
            }
            KeyCode::Char('u') => {
                self.input_panel_state.textarea.undo();
            }
            KeyCode::Char('~') => {
                let pos = self.input_panel_state.textarea.cursor();
                let lines = self.input_panel_state.textarea.lines();
                if let Some(line) = lines.get(pos.0) {
                    if let Some(ch) = line.chars().nth(pos.1) {
                        self.input_panel_state.textarea.delete_next_char();
                        let toggled = if ch.is_uppercase() {
                            ch.to_lowercase().to_string()
                        } else {
                            ch.to_uppercase().to_string()
                        };
                        self.input_panel_state.textarea.insert_str(&toggled);
                    }
                }
            }
            KeyCode::Char('J') => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                let pos = self.input_panel_state.textarea.cursor();
                let lines = self.input_panel_state.textarea.lines();
                if pos.0 < lines.len() - 1 {
                    self.input_panel_state.textarea.delete_next_char();
                    self.input_panel_state.textarea.insert_char(' ');
                }
            }

            // Movement
            KeyCode::Char('h') | KeyCode::Left => self
                .input_panel_state
                .textarea
                .move_cursor(CursorMove::Back),
            KeyCode::Char('l') | KeyCode::Right => self
                .input_panel_state
                .textarea
                .move_cursor(CursorMove::Forward),
            KeyCode::Char('j') | KeyCode::Down => self.chat_viewport.state_mut().scroll_down(1),
            KeyCode::Char('k') | KeyCode::Up => self.chat_viewport.state_mut().scroll_up(1),
            KeyCode::Char('w') => self
                .input_panel_state
                .textarea
                .move_cursor(CursorMove::WordForward),
            KeyCode::Char('b') => self
                .input_panel_state
                .textarea
                .move_cursor(CursorMove::WordBack),
            KeyCode::Char('0') | KeyCode::Char('^') => self
                .input_panel_state
                .textarea
                .move_cursor(CursorMove::Head),
            KeyCode::Char('$') => self.input_panel_state.textarea.move_cursor(CursorMove::End),
            KeyCode::Char('G') => self.chat_viewport.state_mut().scroll_to_bottom(),
            KeyCode::Char('g') => {
                if self.vim_state.pending_g {
                    self.chat_viewport.state_mut().scroll_to_top();
                }
                self.vim_state.pending_g = !self.vim_state.pending_g;
                should_clear_state = false;
            }

            // Visual mode
            KeyCode::Char('v') => {
                self.input_panel_state.textarea.start_selection();
                self.vim_state.visual_mode = true;
            }
            KeyCode::Char('V') => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Head);
                self.input_panel_state.textarea.start_selection();
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                self.vim_state.visual_mode = true;
            }

            // Replace mode
            KeyCode::Char('r') => {
                self.vim_state.replace_mode = true;
                should_clear_state = false;
            }
            KeyCode::Char(ch) if self.vim_state.replace_mode => {
                self.input_panel_state.textarea.delete_next_char();
                self.input_panel_state.textarea.insert_char(ch);
                self.vim_state.replace_mode = false;
            }

            // Other commands
            KeyCode::Char('e') => self.enter_edit_selection_mode(),
            KeyCode::Char('/') => {
                // Switch to command mode with fuzzy finder like Simple/VimInsert modes
                self.input_panel_state.clear();
                self.input_panel_state.insert_str("/");
                // Activate command fuzzy finder immediately
                self.input_panel_state.activate_command_fuzzy();
                self.switch_mode(InputMode::FuzzyFinder);

                // Populate results with all commands initially
                let results: Vec<String> = self
                    .command_registry
                    .all_commands()
                    .into_iter()
                    .map(|cmd| cmd.name.to_string())
                    .collect();
                self.input_panel_state.fuzzy_finder.update_results(results);
            }
            KeyCode::Char('!') => {
                self.input_panel_state.clear();
                self.input_panel_state
                    .textarea
                    .set_placeholder_text("Enter bash command...");
                self.switch_mode(InputMode::BashCommand);
            }

            _ => {
                should_clear_state = false;
            }
        }

        if should_clear_state {
            self.vim_state.pending_g = false;
            self.vim_state.pending_operator = None;
        }

        Ok(false)
    }

    async fn handle_vim_insert(&mut self, key: KeyEvent) -> Result<bool> {
        // Try common text manipulation first
        if self.handle_text_manipulation(key)? {
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                // Check for double-tap to clear
                if self
                    .double_tap_tracker
                    .is_double_tap(KeyCode::Esc, Duration::from_millis(300))
                {
                    // Double ESC - clear content
                    self.input_panel_state.clear();
                    self.double_tap_tracker.clear_key(&KeyCode::Esc);
                } else {
                    // Single ESC - return to normal mode
                    self.double_tap_tracker.record_key(KeyCode::Esc);
                    self.set_mode(InputMode::VimNormal);
                    // Move cursor back one position (vim behavior)
                    self.input_panel_state
                        .textarea
                        .move_cursor(CursorMove::Back);
                }
            }

            KeyCode::Enter => {
                let content = self.input_panel_state.content().trim().to_string();
                if !content.is_empty() {
                    if content.starts_with('!') && content.len() > 1 {
                        // Execute as bash command
                        let command = content[1..].trim().to_string();
                        self.command_sink
                            .send_command(AppCommand::ExecuteBashCommand { command })
                            .await?;
                        self.input_panel_state.clear();
                        self.set_mode(InputMode::VimNormal);
                    } else if content.starts_with('/') {
                        // Handle as slash command
                        let old_editing_mode = self.preferences.ui.editing_mode;
                        self.handle_slash_command(content).await?;
                        self.input_panel_state.clear();
                        // Return to VimNormal only if we're *still* in VimInsert and the
                        // editing mode hasn’t changed (e.g. not switched into Setup).
                        if self.input_mode == InputMode::VimInsert
                            && self.preferences.ui.editing_mode == old_editing_mode
                        {
                            self.set_mode(InputMode::VimNormal);
                        }
                    } else {
                        // Send as normal message
                        self.send_message(content).await?;
                        self.input_panel_state.clear();
                        self.set_mode(InputMode::VimNormal);
                    }
                } else {
                    // Just insert a newline if empty
                    self.input_panel_state.handle_input(Input::from(key));
                }
            }

            KeyCode::Char('!') => {
                let content = self.input_panel_state.content();
                if content.is_empty() {
                    // First character - enter bash command mode without inserting '!'
                    self.input_panel_state
                        .textarea
                        .set_placeholder_text("Enter bash command...");
                    self.switch_mode(InputMode::BashCommand);
                } else {
                    // Normal ! character
                    self.input_panel_state.handle_input(Input::from(key));
                }
            }

            KeyCode::Char('@') => {
                // Activate file fuzzy finder
                self.input_panel_state.handle_input(Input::from(key));
                self.input_panel_state.activate_fuzzy();
                self.switch_mode(InputMode::FuzzyFinder);

                // Immediately show all files (limited to 20)
                let results = self
                    .input_panel_state
                    .file_cache()
                    .fuzzy_search("", Some(20))
                    .await;
                self.input_panel_state.fuzzy_finder.update_results(results);
            }

            KeyCode::Char('/') => {
                let content = self.input_panel_state.content();
                if content.is_empty() {
                    // Activate command fuzzy finder
                    self.input_panel_state.handle_input(Input::from(key));
                    self.input_panel_state.activate_command_fuzzy();
                    self.switch_mode(InputMode::FuzzyFinder);

                    // Immediately show all commands
                    let results: Vec<String> = self
                        .command_registry
                        .all_commands()
                        .into_iter()
                        .map(|cmd| cmd.name.to_string())
                        .collect();
                    self.input_panel_state.fuzzy_finder.update_results(results);
                } else {
                    // Normal / character
                    self.input_panel_state.handle_input(Input::from(key));
                }
            }

            // Quick escape to normal mode
            KeyCode::Char('[') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_mode(InputMode::VimNormal);
                // Move cursor back one position (vim behavior)
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Back);
            }

            _ => {
                // Normal text input
                self.input_panel_state.handle_input(Input::from(key));
            }
        }
        Ok(false)
    }
}
