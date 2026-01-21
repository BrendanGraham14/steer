use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, Input};

impl Tui {
    /// Common text manipulation handler used by both Simple mode and Vim insert mode
    pub fn handle_text_manipulation(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            // Basic navigation
            (KeyCode::Left, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Back);
                Ok(true)
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Forward);
                Ok(true)
            }
            (KeyCode::Up, KeyModifiers::NONE) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::Up);
                Ok(true)
            }
            (KeyCode::Down, KeyModifiers::NONE) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Down);
                Ok(true)
            }

            // Word navigation (Alt/Option + arrows)
            (KeyCode::Left, KeyModifiers::ALT) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::WordBack);
                Ok(true)
            }
            (KeyCode::Right, KeyModifiers::ALT) => {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::WordForward);
                Ok(true)
            }

            // Line navigation (Cmd/Ctrl + arrows, Home/End, Ctrl+A/Ctrl+E)
            (KeyCode::Left, m)
                if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
            {
                self.input_panel_state
                    .textarea
                    .move_cursor(CursorMove::Head);
                Ok(true)
            }
            (KeyCode::Right, m)
                if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
            {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                Ok(true)
            }
            (KeyCode::Home, _) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::Head);
                Ok(true)
            }
            (KeyCode::End, _) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                Ok(true)
            }
            (KeyCode::Char('a'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::Head);
                Ok(true)
            }
            (KeyCode::Char('e'), m) if m.contains(KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.move_cursor(CursorMove::End);
                Ok(true)
            }

            // Text deletion
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_head();
                Ok(true)
            }
            (KeyCode::Char('k'), KeyModifiers::CONTROL) => {
                self.input_panel_state.textarea.delete_line_by_end();
                Ok(true)
            }
            (KeyCode::Backspace, m)
                if m.contains(KeyModifiers::SUPER) || m.contains(KeyModifiers::CONTROL) =>
            {
                self.input_panel_state.textarea.delete_line_by_head();
                Ok(true)
            }
            (KeyCode::Delete, m)
                if m.contains(KeyModifiers::SUPER) || m.contains(KeyModifiers::CONTROL) =>
            {
                self.input_panel_state.textarea.delete_line_by_end();
                Ok(true)
            }

            // Word deletion
            (KeyCode::Backspace, KeyModifiers::ALT) => {
                self.input_panel_state.textarea.delete_word();
                Ok(true)
            }

            // Multi-line support
            (KeyCode::Enter, m)
                if m.contains(KeyModifiers::SHIFT)
                    || m.contains(KeyModifiers::ALT)
                    || m.contains(KeyModifiers::CONTROL) =>
            {
                self.input_panel_state
                    .handle_input(Input::from(KeyEvent::new(
                        KeyCode::Char('\n'),
                        KeyModifiers::empty(),
                    )));
                Ok(true)
            }
            (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
                self.input_panel_state
                    .handle_input(Input::from(KeyEvent::new(
                        KeyCode::Char('\n'),
                        KeyModifiers::empty(),
                    )));
                Ok(true)
            }

            // Not handled by text manipulation
            _ => Ok(false),
        }
    }
}
