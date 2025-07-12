use crate::error::Result;
use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode;
use crate::tui::{InputMode, Tui};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::Input;

impl Tui {
    pub async fn handle_fuzzy_finder_mode(&mut self, key: KeyEvent) -> Result<bool> {
        use crate::tui::widgets::fuzzy_finder::FuzzyFinderResult;

        // Handle various newline key combinations
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

        // Get the current mode
        let mode = self.input_panel_state.fuzzy_finder.mode();

        // First, let the input panel process the key
        let post_result = self.input_panel_state.handle_fuzzy_key(key).await;

        // Determine if cursor is still immediately after trigger character
        let cursor_after_trigger = {
            let content = self.input_panel_state.content();
            let (row, col) = self.input_panel_state.textarea.cursor();
            // Get absolute byte offset of cursor by summing line lengths + newlines
            let mut offset = 0usize;
            for (i, line) in self.input_panel_state.textarea.lines().iter().enumerate() {
                if i == row {
                    offset += col;
                    break;
                } else {
                    offset += line.len() + 1;
                }
            }
            // Check if we have a stored trigger position
            if let Some(trigger_pos) = self.input_panel_state.fuzzy_finder.trigger_position() {
                // Check if cursor is past the trigger and no whitespace between
                if offset <= trigger_pos {
                    false // Cursor before the trigger
                } else {
                    let bytes = content.as_bytes();
                    // Check for whitespace between trigger and cursor
                    let mut still_in_word = true;
                    for idx in trigger_pos + 1..offset {
                        if idx >= bytes.len() {
                            break;
                        }
                        match bytes[idx] {
                            b' ' | b'\t' | b'\n' => {
                                still_in_word = false;
                                break;
                            }
                            _ => {}
                        }
                    }
                    still_in_word
                }
            } else {
                false // No trigger position stored
            }
        };

        if !cursor_after_trigger {
            self.input_panel_state.deactivate_fuzzy();
            self.input_mode = InputMode::Insert;
            return Ok(false);
        }

        // Otherwise handle explicit results (Enter / Esc etc.)
        if let Some(result) = post_result {
            match result {
                FuzzyFinderResult::Close => {
                    self.input_panel_state.deactivate_fuzzy();
                    self.input_mode = InputMode::Insert;
                }
                FuzzyFinderResult::Select(selected) => {
                    match mode {
                        FuzzyFinderMode::Files => {
                            // Complete with file path
                            self.input_panel_state.complete_fuzzy_finder(&selected);
                        }
                        FuzzyFinderMode::Commands => {
                            // Complete with command
                            self.input_panel_state.complete_command_fuzzy(&selected);
                        }
                    }
                    self.input_panel_state.deactivate_fuzzy();
                    self.input_mode = InputMode::Insert;
                }
            }
        }

        // Handle typing for command search
        if mode == FuzzyFinderMode::Commands {
            // Extract search query from content
            let content = self.input_panel_state.content();
            if let Some(trigger_pos) = self.input_panel_state.fuzzy_finder.trigger_position() {
                if trigger_pos + 1 < content.len() {
                    let query = &content[trigger_pos + 1..];
                    // Search commands
                    let results: Vec<String> = self
                        .command_registry
                        .search(query)
                        .into_iter()
                        .map(|cmd| cmd.name.to_string())
                        .collect();
                    self.input_panel_state.fuzzy_finder.update_results(results);
                }
            }
        }

        Ok(false)
    }
}
