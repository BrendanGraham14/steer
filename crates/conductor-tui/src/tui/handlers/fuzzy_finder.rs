use crate::tui::{InputMode, Tui};
use anyhow::Result;
use crossterm::event::KeyEvent;

impl Tui {
    pub async fn handle_fuzzy_finder_mode(&mut self, key: KeyEvent) -> Result<bool> {
        use crate::tui::widgets::fuzzy_finder::FuzzyFinderResult;

        // First, let the input panel process the key
        let post_result = self.input_panel_state.handle_fuzzy_key(key).await;

        // Determine if cursor is still immediately after an @ with no whitespace in-between
        let cursor_after_at = {
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
            if let Some(at_pos) = self.input_panel_state.fuzzy_finder.trigger_position() {
                // Check if cursor is past the @ and no whitespace between
                if offset <= at_pos {
                    false // Cursor before the @
                } else {
                    let bytes = content.as_bytes();
                    // Check for whitespace between @ and cursor
                    let mut still_in_word = true;
                    for idx in at_pos + 1..offset {
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

        if !cursor_after_at {
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
                FuzzyFinderResult::Select(path) => {
                    self.input_panel_state.complete_fuzzy_finder(&path);
                    self.input_panel_state.deactivate_fuzzy();
                    self.input_mode = InputMode::Insert;
                }
            }
        }
        Ok(false)
    }
}
