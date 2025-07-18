use ratatui::layout::Rect;
use ratatui::prelude::{Buffer, StatefulWidget, Widget};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState,
};
use tui_textarea::{Input, TextArea};

use conductor_core::app::conversation::{Message, UserContent};
use conductor_tools::schema::ToolCall;

use crate::tui::InputMode;
use crate::tui::get_spinner_char;
use crate::tui::model::ChatItem;
use crate::tui::state::file_cache::FileCache;
use crate::tui::theme::{Component, Theme};
use crate::tui::widgets::fuzzy_finder::{FuzzyFinder, FuzzyFinderMode};

/// Helper function to format keybind hints with consistent styling
fn format_keybind(key: &str, description: &str, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("[{key}]"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {description}"), theme.style(Component::DimText)),
    ]
}

fn format_keybinds(keybinds: &[(&str, &str)], theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, (key, description)) in keybinds.iter().enumerate() {
        spans.extend(format_keybind(key, description, theme));
        if i < keybinds.len() - 1 {
            spans.push(Span::styled(" │ ", theme.style(Component::DimText)));
        }
    }
    spans
}

/// Stateful data for the [`InputPanel`] widget.
#[derive(Debug)]
pub struct InputPanelState {
    pub textarea: TextArea<'static>,
    pub edit_selection_messages: Vec<(String, String)>,
    pub edit_selection_index: usize,
    pub edit_selection_hovered_id: Option<String>,
    /// File cache for fuzzy finder
    pub file_cache: FileCache,
    /// Fuzzy finder widget
    pub fuzzy_finder: FuzzyFinder,
}

impl Default for InputPanelState {
    fn default() -> Self {
        // For tests and default usage, use a dummy session ID
        Self::new("default".to_string())
    }
}

impl InputPanelState {
    /// Create a new InputPanelState with the given session ID
    pub fn new(session_id: String) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Type your message here...");
        textarea.set_cursor_line_style(Style::default());
        textarea.set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
        Self {
            textarea,
            edit_selection_messages: Vec::new(),
            edit_selection_index: 0,
            edit_selection_hovered_id: None,
            file_cache: FileCache::new(session_id),
            fuzzy_finder: FuzzyFinder::new(),
        }
    }

    /// Get the byte offset of the cursor in the textarea content.
    pub fn get_cursor_byte_offset(&self) -> usize {
        let (row, col) = self.textarea.cursor();
        let lines = self.textarea.lines();
        let mut offset = 0;
        for (i, line) in lines.iter().enumerate() {
            if i < row {
                offset += line.len() + 1; // +1 for newline
            } else {
                // `col` is a grapheme cluster count, find the byte offset for that.
                offset += line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
                break;
            }
        }
        offset
    }

    /// Checks if the fuzzy finder is active and the cursor is in a valid query position.
    /// This method does not allocate and is suitable for checks on every tick.
    pub fn is_in_fuzzy_query(&self) -> bool {
        if !self.fuzzy_finder.is_active() {
            return false;
        }

        let Some(at_pos) = self.fuzzy_finder.trigger_position() else {
            return false;
        };

        let cursor_offset = self.get_cursor_byte_offset();
        if cursor_offset <= at_pos {
            return false; // Cursor is before or on the trigger character
        }

        let content = self.content();
        // The part of the string that could be the query
        let query_candidate = &content[at_pos + 1..cursor_offset];

        // If it contains whitespace, it's not a valid query anymore
        !query_candidate.chars().any(char::is_whitespace)
    }

    /// If the fuzzy finder is active and the cursor is in a valid query position,
    /// returns the query string. Otherwise, returns None.
    pub fn get_current_fuzzy_query(&self) -> Option<String> {
        if self.is_in_fuzzy_query() {
            let at_pos = self.fuzzy_finder.trigger_position().unwrap(); // Safe due to check above
            let cursor_offset = self.get_cursor_byte_offset();
            let content = self.content();
            let query_candidate = &content[at_pos + 1..cursor_offset];
            Some(query_candidate.to_string())
        } else {
            None
        }
    }

    /// Handle input in insert/bash modes
    pub fn handle_input(&mut self, input: Input) {
        self.textarea.input(input);
    }

    /// Complete fuzzy finder by replacing the query text with the selected path
    pub fn complete_fuzzy_finder(&mut self, selected_path: &str) {
        if let Some(at_pos) = self.fuzzy_finder.trigger_position() {
            let cursor_offset = self.get_cursor_byte_offset();

            // Convert content to string and replace the query portion
            let content = self.content();
            let mut new_content = String::new();

            // Keep everything up to and including the @
            new_content.push_str(&content[..=at_pos]);

            // Add the selected path and a space
            new_content.push_str(selected_path);
            new_content.push(' ');

            // Keep everything after the cursor
            if cursor_offset < content.len() {
                new_content.push_str(&content[cursor_offset..]);
            }

            // Replace the entire content
            let lines: Vec<&str> = new_content.lines().collect();
            self.set_content_from_lines(lines);

            // Position cursor after the inserted path and space (which is a byte position)
            let new_cursor_pos_bytes = at_pos + 1 + selected_path.len() + 1;

            // Now, convert this byte position to a (row, col) grapheme position
            let mut bytes_traversed = 0;
            for (row_idx, line) in self.textarea.lines().iter().enumerate() {
                let line_len_bytes = line.len();
                if bytes_traversed + line_len_bytes >= new_cursor_pos_bytes {
                    // The cursor should be on this line
                    let byte_offset_in_line = new_cursor_pos_bytes - bytes_traversed;
                    // Convert byte offset in line to character/grapheme column
                    let char_col = line[..byte_offset_in_line].chars().count();
                    self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
                        row_idx as u16,
                        char_col as u16,
                    ));
                    break;
                }
                bytes_traversed += line_len_bytes + 1; // +1 for newline
            }
        }
    }

    /// Complete command fuzzy finder by replacing the query text with the selected command
    pub fn complete_command_fuzzy(&mut self, selected_command: &str) {
        if let Some(trigger_pos) = self.fuzzy_finder.trigger_position() {
            let cursor_offset = self.get_cursor_byte_offset();

            // Convert content to string and replace the query portion
            let content = self.content();
            let mut new_content = String::new();

            // For commands, we want to replace everything from the trigger onwards
            new_content.push_str(&content[..trigger_pos]);

            // Add the selected command with a space
            new_content.push('/');
            new_content.push_str(selected_command);
            new_content.push(' ');

            // Keep everything after the cursor
            if cursor_offset < content.len() {
                new_content.push_str(&content[cursor_offset..]);
            }

            // Replace the entire content
            let lines: Vec<&str> = new_content.lines().collect();
            self.set_content_from_lines(lines);

            // Position cursor after the inserted command and space
            let new_cursor_pos_bytes = trigger_pos + 1 + selected_command.len() + 1;

            // Now, convert this byte position to a (row, col) grapheme position
            let mut bytes_traversed = 0;
            for (row_idx, line) in self.textarea.lines().iter().enumerate() {
                let line_len_bytes = line.len();
                if bytes_traversed + line_len_bytes >= new_cursor_pos_bytes {
                    // The cursor should be on this line
                    let byte_offset_in_line = new_cursor_pos_bytes - bytes_traversed;
                    // Convert byte offset in line to character/grapheme column
                    let char_col = line[..byte_offset_in_line].chars().count();
                    self.textarea.move_cursor(tui_textarea::CursorMove::Jump(
                        row_idx as u16,
                        char_col as u16,
                    ));
                    break;
                }
                bytes_traversed += line_len_bytes + 1; // +1 for newline
            }
        }
    }

    /// Insert a string (e.g., for paste operations)
    pub fn insert_str(&mut self, s: &str) {
        self.textarea.insert_str(s);
    }

    /// Get the current content as a single string
    pub fn content(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Clear the textarea
    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.textarea
            .set_placeholder_text("Type your message here...");
        self.textarea.set_cursor_line_style(Style::default());
        self.textarea
            .set_cursor_style(Style::default().add_modifier(Modifier::REVERSED));
    }

    /// Set content from lines (used when editing a message)
    pub fn set_content_from_lines(&mut self, lines: Vec<&str>) {
        self.textarea = TextArea::from(lines);
    }

    /// Calculate required height for the input panel
    pub fn required_height(
        &self,
        current_approval: Option<&ToolCall>,
        width: u16,
        max_height: u16,
    ) -> u16 {
        if let Some(tool_call) = current_approval {
            // If there's a pending approval, use the approval height calculation
            Self::required_height_for_approval(tool_call, width, max_height)
        } else {
            // Otherwise use the regular calculation based on textarea lines
            let line_count = self.textarea.lines().len().max(1);
            // line count + 2 for borders + 1 for padding
            (line_count + 3).min(max_height as usize) as u16
        }
    }

    /// Calculate required height for approval mode
    pub fn required_height_for_approval(tool_call: &ToolCall, width: u16, max_height: u16) -> u16 {
        let theme = &Theme::default();
        let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
        let preview_lines = formatter.compact(
            &tool_call.parameters,
            &None,
            width.saturating_sub(4) as usize,
            theme,
        );
        // 2 lines for header + preview lines + 2 for borders + 1 for padding
        (2 + preview_lines.len() + 3).min(max_height as usize) as u16
    }

    /// Navigate up in edit selection mode
    pub fn edit_selection_prev(&mut self) -> Option<&(String, String)> {
        if self.edit_selection_index > 0 {
            self.edit_selection_index -= 1;
            self.update_hovered_id();
            self.edit_selection_messages.get(self.edit_selection_index)
        } else {
            self.edit_selection_messages.get(self.edit_selection_index)
        }
    }

    /// Navigate down in edit selection mode
    pub fn edit_selection_next(&mut self) -> Option<&(String, String)> {
        if self.edit_selection_index + 1 < self.edit_selection_messages.len() {
            self.edit_selection_index += 1;
            self.update_hovered_id();
            self.edit_selection_messages.get(self.edit_selection_index)
        } else {
            self.edit_selection_messages.get(self.edit_selection_index)
        }
    }

    /// Get currently selected message in edit selection mode
    pub fn get_selected_message(&self) -> Option<&(String, String)> {
        self.edit_selection_messages.get(self.edit_selection_index)
    }

    /// Populate edit selection messages from chat store
    pub fn populate_edit_selection<'a>(&mut self, chat_items: impl Iterator<Item = &'a ChatItem>) {
        self.edit_selection_messages = chat_items
            .filter_map(|item| {
                if let ChatItem::Message(row) = item {
                    if let Message::User { content, .. } = &row.inner {
                        // Extract text content from user blocks
                        let text = content
                            .iter()
                            .filter_map(|block| match block {
                                UserContent::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Some((row.inner.id().to_string(), text))
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Select the last (most recent) message if available
        if !self.edit_selection_messages.is_empty() {
            self.edit_selection_index = self.edit_selection_messages.len() - 1;
            self.update_hovered_id();
        } else {
            self.edit_selection_index = 0;
            self.edit_selection_hovered_id = None;
        }
    }

    /// Update the hovered message ID based on current selection
    fn update_hovered_id(&mut self) {
        self.edit_selection_hovered_id = self.get_selected_message().map(|(id, _)| id.clone());
    }

    /// Get the current hovered message ID
    pub fn get_hovered_id(&self) -> Option<&str> {
        self.edit_selection_hovered_id.as_deref()
    }

    /// Clear edit selection state
    pub fn clear_edit_selection(&mut self) {
        self.edit_selection_messages.clear();
        self.edit_selection_index = 0;
        self.edit_selection_hovered_id = None;
    }

    /// Activate fuzzy finder for files
    pub fn activate_fuzzy(&mut self) {
        // The @ is one character before the cursor (since we just typed it)
        let cursor_pos = self.get_cursor_byte_offset();
        if cursor_pos > 0 {
            // The trigger is the @ just before the cursor
            self.fuzzy_finder
                .activate(cursor_pos - 1, FuzzyFinderMode::Files);
        } else {
            // Shouldn't happen, but handle gracefully
            self.fuzzy_finder.activate(0, FuzzyFinderMode::Files);
        }
    }

    /// Activate fuzzy finder for commands
    pub fn activate_command_fuzzy(&mut self) {
        // The / is one character before the cursor (since we just typed it)
        let cursor_pos = self.get_cursor_byte_offset();
        if cursor_pos > 0 {
            // The trigger is the / just before the cursor
            self.fuzzy_finder
                .activate(cursor_pos - 1, FuzzyFinderMode::Commands);
        } else {
            // Shouldn't happen, but handle gracefully
            self.fuzzy_finder.activate(0, FuzzyFinderMode::Commands);
        }
    }

    /// Deactivate fuzzy finder
    pub fn deactivate_fuzzy(&mut self) {
        self.fuzzy_finder.deactivate();
    }

    /// Check if fuzzy finder is active
    pub fn fuzzy_active(&self) -> bool {
        self.fuzzy_finder.is_active()
    }

    /// Handle key event for fuzzy finder
    pub async fn handle_fuzzy_key(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
    ) -> Option<crate::tui::widgets::fuzzy_finder::FuzzyFinderResult> {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        use tui_textarea::{CursorMove, Input};

        // First handle navigation/selection in the fuzzy finder itself
        let result = self.fuzzy_finder.handle_input(key);

        if result.is_some() {
            // Key was handled (e.g., selection, closing), so just return the result
            return result;
        }

        // Block up/down arrows from reaching the textarea when fuzzy finder is active
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                // These keys are for fuzzy finder navigation only
                return None;
            }
            _ => {}
        }

        // Handle Alt+Left/Right for word navigation
        if key.modifiers == KeyModifiers::ALT {
            match key.code {
                KeyCode::Left => {
                    self.textarea.move_cursor(CursorMove::WordBack);
                    return None;
                }
                KeyCode::Right => {
                    self.textarea.move_cursor(CursorMove::WordForward);
                    return None;
                }
                _ => {}
            }
        }

        // Key was not for navigation, so treat it as text input
        let input = Input::from(key);
        self.textarea.input(input);

        // After input, handle result updates based on the active fuzzy finder mode.
        use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode;

        if self.fuzzy_finder.mode() == FuzzyFinderMode::Files {
            // For file search, update results or close if the query is no longer valid
            if let Some(query) = self.get_current_fuzzy_query() {
                let results = self.file_cache.fuzzy_search(&query, Some(10)).await;
                self.fuzzy_finder.update_results(results);
                None // Query updated; stay active
            } else {
                // Query became invalid (e.g., whitespace typed) – request close
                Some(crate::tui::widgets::fuzzy_finder::FuzzyFinderResult::Close)
            }
        } else {
            // For other modes (Commands, Models, Themes) the Tui handler deals with closing logic.
            None
        }
    }

    /// Get file cache reference
    pub fn file_cache(&self) -> &FileCache {
        &self.file_cache
    }

    /// Get mutable file cache reference
    pub fn file_cache_mut(&mut self) -> &mut FileCache {
        &mut self.file_cache
    }
}

fn get_formatted_mode(mode: InputMode, theme: &Theme) -> Option<Span<'static>> {
    // Add mode name with special styling
    let mode_name = match mode {
        InputMode::Simple => return None,
        InputMode::VimNormal => "NORMAL",
        InputMode::VimInsert => "INSERT",
        InputMode::BashCommand => "Bash",
        InputMode::AwaitingApproval => "Awaiting Approval",
        InputMode::ConfirmExit => "Confirm Exit",
        InputMode::EditMessageSelection => "Edit Selection",
        InputMode::FuzzyFinder => "Search",
        InputMode::Setup => "Setup",
    };

    let component = match mode {
        InputMode::ConfirmExit => Component::ErrorBold,
        InputMode::BashCommand => Component::CommandPrompt,
        InputMode::AwaitingApproval => Component::ErrorBold,
        InputMode::EditMessageSelection => Component::SelectionHighlight,
        InputMode::FuzzyFinder => Component::SelectionHighlight,
        _ => Component::ModelInfo,
    };

    Some(Span::styled(mode_name, theme.style(component)))
}

/// Properties for the [`InputPanel`] widget.
#[derive(Clone, Copy, Debug)]
pub struct InputPanel<'a> {
    pub input_mode: InputMode,
    pub current_approval: Option<&'a ToolCall>,
    pub is_processing: bool,
    pub spinner_state: usize,
    pub theme: &'a Theme,
}

impl<'a> InputPanel<'a> {
    pub fn new(
        input_mode: InputMode,
        current_approval: Option<&'a ToolCall>,
        is_processing: bool,
        spinner_state: usize,
        theme: &'a Theme,
    ) -> Self {
        Self {
            input_mode,
            current_approval,
            is_processing,
            spinner_state,
            theme,
        }
    }

    /// Get the title for the current mode with properly formatted keybinds
    fn get_mode_title(&self, state: &InputPanelState) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];

        let formatted_mode = get_formatted_mode(self.input_mode, self.theme);
        if let Some(mode) = formatted_mode {
            spans.push(mode);
            spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
        }

        match self.input_mode {
            InputMode::Simple => {
                if state.content().is_empty() {
                    spans.extend(format_keybinds(
                        &[
                            ("Enter", "send"),
                            ("ESC ESC", "edit previous"),
                            ("!", "bash"),
                            ("/", "command"),
                            ("@", "file"),
                        ],
                        self.theme,
                    ));
                } else {
                    spans.extend(format_keybinds(
                        &[("Enter", "send"), ("ESC ESC", "clear")],
                        self.theme,
                    ));
                }
            }
            InputMode::VimNormal => {
                if state.content().is_empty() {
                    spans.extend(format_keybinds(
                        &[
                            ("i", "insert"),
                            ("ESC ESC", "edit previous"),
                            ("!", "bash"),
                            ("/", "command"),
                        ],
                        self.theme,
                    ));
                } else {
                    spans.extend(format_keybinds(
                        &[("i", "insert"), ("ESC ESC", "clear"), ("hjkl", "move")],
                        self.theme,
                    ));
                }
            }
            InputMode::VimInsert => {
                spans.extend(format_keybinds(
                    &[("Esc", "normal"), ("ESC ESC", "clear"), ("Enter", "send")],
                    self.theme,
                ));
            }
            InputMode::BashCommand => {
                spans.extend(format_keybinds(
                    &[("Enter", "execute"), ("Esc", "cancel")],
                    self.theme,
                ));
            }
            InputMode::AwaitingApproval => {
                // No keybinds for this mode
            }
            InputMode::ConfirmExit => {
                spans.extend(format_keybinds(
                    &[("y/Y", "confirm"), ("any other key", "cancel")],
                    self.theme,
                ));
            }
            InputMode::EditMessageSelection => {
                spans.extend(format_keybinds(
                    &[("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")],
                    self.theme,
                ));
            }
            InputMode::FuzzyFinder => {
                spans.extend(format_keybinds(
                    &[("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")],
                    self.theme,
                ));
            }
            InputMode::Setup => {
                // No keybinds shown during setup mode
            }
        }

        spans.push(Span::raw(" "));
        Line::from(spans)
    }
}

impl StatefulWidget for InputPanel<'_> {
    type State = InputPanelState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Render approval prompt if needed
        if let Some(tool_call) = self.current_approval {
            let formatter = crate::tui::widgets::formatters::get_formatter(&tool_call.name);
            let preview_lines = formatter.compact(
                &tool_call.parameters,
                &None,
                (area.width.saturating_sub(4)) as usize,
                self.theme,
            );

            let is_bash_command = tool_call.name == "bash";

            let mut approval_text = if is_bash_command {
                vec![
                    Line::from(vec![
                        Span::styled("Tool ", Style::default()),
                        Span::styled(&tool_call.name, self.theme.style(Component::ToolCallHeader)),
                        Span::styled(" wants to run this shell command", Style::default()),
                    ]),
                    Line::from(""),
                ]
            } else {
                vec![
                    Line::from(vec![
                        Span::styled("Tool ", Style::default()),
                        Span::styled(&tool_call.name, self.theme.style(Component::ToolCallHeader)),
                        Span::styled(" needs your approval", Style::default()),
                    ]),
                    Line::from(""),
                ]
            };
            approval_text.extend(preview_lines);

            let approval_keybinds = if is_bash_command {
                vec![
                    (
                        Span::styled("[Y]", self.theme.style(Component::ToolSuccess)),
                        Span::styled("Yes (once)", self.theme.style(Component::DimText)),
                    ),
                    (
                        Span::styled("[A]", self.theme.style(Component::ToolSuccess)),
                        Span::styled(
                            "Always (this command)",
                            self.theme.style(Component::DimText),
                        ),
                    ),
                    (
                        Span::styled("[L]", self.theme.style(Component::ToolSuccess)),
                        Span::styled(
                            "Always (all Bash commands)",
                            self.theme.style(Component::DimText),
                        ),
                    ),
                    (
                        Span::styled("[N]", self.theme.style(Component::ToolError)),
                        Span::styled("No", self.theme.style(Component::DimText)),
                    ),
                ]
            } else {
                vec![
                    (
                        Span::styled("[Y]", self.theme.style(Component::ToolSuccess)),
                        Span::styled("Yes (once)", self.theme.style(Component::DimText)),
                    ),
                    (
                        Span::styled("[A]", self.theme.style(Component::ToolSuccess)),
                        Span::styled("Always", self.theme.style(Component::DimText)),
                    ),
                    (
                        Span::styled("[N]", self.theme.style(Component::ToolError)),
                        Span::styled("No", self.theme.style(Component::DimText)),
                    ),
                ]
            };

            let mut title_spans = vec![Span::raw(" Approval Required "), Span::raw("─ ")];

            for (i, (key, desc)) in approval_keybinds.iter().enumerate() {
                if i > 0 {
                    title_spans.push(Span::styled(" │ ", self.theme.style(Component::DimText)));
                }
                title_spans.push(key.clone());
                title_spans.push(Span::raw(" "));
                title_spans.push(desc.clone());
            }
            title_spans.push(Span::raw(" "));

            let title = Line::from(title_spans);

            let approval_block = Paragraph::new(approval_text).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .style(self.theme.style(Component::InputPanelBorderApproval)),
            );

            approval_block.render(area, buf);
            return;
        }

        // Normal input / edit selection rendering
        let mut title_spans = vec![];

        // Add spinner if processing
        if self.is_processing {
            title_spans.push(Span::styled(
                format!(" {}", get_spinner_char(self.spinner_state)),
                self.theme.style(Component::ToolCall),
            ));
        }

        // Add mode-specific title
        title_spans.extend(self.get_mode_title(state).spans);

        let mut input_block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title_spans));

        match self.input_mode {
            InputMode::Simple | InputMode::VimInsert => {
                // Active border and text style
                let active = self.theme.style(Component::InputPanelBorderActive);
                input_block = input_block.style(active).border_style(active);
            }
            InputMode::VimNormal => {
                // Keep text style the same as VimInsert (active) but dim the border
                let text_style = self.theme.style(Component::InputPanelBorderActive);
                let border_dim = self.theme.style(Component::InputPanelBorder);
                input_block = input_block.style(text_style).border_style(border_dim);
            }
            InputMode::BashCommand => {
                let style = self.theme.style(Component::InputPanelBorderCommand);
                input_block = input_block.style(style).border_style(style);
            }
            InputMode::ConfirmExit => {
                let style = self.theme.style(Component::InputPanelBorderError);
                input_block = input_block.style(style).border_style(style);
            }
            InputMode::EditMessageSelection => {
                let style = self.theme.style(Component::InputPanelBorderCommand);
                input_block = input_block.style(style).border_style(style);
            }
            InputMode::FuzzyFinder => {
                let style = self.theme.style(Component::InputPanelBorderActive);
                input_block = input_block.style(style).border_style(style);
            }
            _ => {
                let style = self.theme.style(Component::InputPanelBorder);
                input_block = input_block.style(style).border_style(style);
            }
        }

        if self.input_mode == InputMode::EditMessageSelection {
            // Selection list rendering
            let mut items: Vec<ListItem> = Vec::new();
            if state.edit_selection_messages.is_empty() {
                items.push(
                    ListItem::new("No user messages to edit")
                        .style(self.theme.style(Component::DimText)),
                );
            } else {
                let max_visible = 3;
                let total = state.edit_selection_messages.len();
                let (start_idx, end_idx) = if total <= max_visible {
                    (0, total)
                } else {
                    let half_window = max_visible / 2;
                    if state.edit_selection_index < half_window {
                        (0, max_visible)
                    } else if state.edit_selection_index >= total - half_window {
                        (total - max_visible, total)
                    } else {
                        let start = state.edit_selection_index - half_window;
                        (start, start + max_visible)
                    }
                };

                for idx in start_idx..end_idx {
                    let (_, content) = &state.edit_selection_messages[idx];
                    let preview = content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(area.width.saturating_sub(4) as usize)
                        .collect::<String>();
                    items.push(ListItem::new(preview));
                }

                let mut list_state = ListState::default();
                list_state.select(Some(state.edit_selection_index.saturating_sub(start_idx)));

                let highlight_style = self
                    .theme
                    .style(Component::SelectionHighlight)
                    .add_modifier(Modifier::REVERSED);

                let list = List::new(items)
                    .block(input_block)
                    .highlight_style(highlight_style);
                StatefulWidget::render(list, area, buf, &mut list_state);
                return;
            }

            // Empty list fallback
            let list = List::new(items).block(input_block);
            Widget::render(list, area, buf);
            return;
        }

        // Default: textarea
        state.textarea.set_block(input_block);
        state.textarea.render(area, buf);

        // Scrollbar when needed
        let textarea_height = area.height.saturating_sub(2);
        let content_lines = state.textarea.lines().len();
        if content_lines > textarea_height as usize {
            let (cursor_row, _) = state.textarea.cursor();
            let mut scrollbar_state = ScrollbarState::new(content_lines)
                .position(cursor_row)
                .viewport_content_length(textarea_height as usize);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"))
                .thumb_style(self.theme.style(Component::DimText));
            let scrollbar_area = Rect {
                x: area.x + area.width - 1,
                y: area.y + 1,
                width: 1,
                height: area.height - 2,
            };
            scrollbar.render(scrollbar_area, buf, &mut scrollbar_state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::model::{ChatItem, MessageRow};

    #[test]
    fn test_input_panel_state_default() {
        let state = InputPanelState::default();
        assert!(state.edit_selection_messages.is_empty());
        assert_eq!(state.edit_selection_index, 0);
        assert!(state.edit_selection_hovered_id.is_none());
        assert_eq!(state.content(), "");
    }

    #[test]
    fn test_input_panel_state_content_operations() {
        let mut state = InputPanelState::default();

        // Test inserting text
        state.insert_str("Hello, world!");
        assert_eq!(state.content(), "Hello, world!");

        // Test clearing
        state.clear();
        assert_eq!(state.content(), "");

        // Test setting content from lines
        state.set_content_from_lines(vec!["Line 1", "Line 2", "Line 3"]);
        assert_eq!(state.content(), "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_edit_selection_navigation() {
        let mut state = InputPanelState {
            edit_selection_messages: vec![
                ("msg1".to_string(), "First message".to_string()),
                ("msg2".to_string(), "Second message".to_string()),
                ("msg3".to_string(), "Third message".to_string()),
            ],
            ..Default::default()
        };
        state.edit_selection_index = 1;
        state.update_hovered_id();

        // Test initial state
        assert_eq!(state.get_hovered_id(), Some("msg2"));

        // Test navigation up
        state.edit_selection_prev();
        assert_eq!(state.edit_selection_index, 0);
        assert_eq!(state.get_hovered_id(), Some("msg1"));

        // Test navigation at boundary
        state.edit_selection_prev();
        assert_eq!(state.edit_selection_index, 0);
        assert_eq!(state.get_hovered_id(), Some("msg1"));

        // Test navigation down
        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 1);
        assert_eq!(state.get_hovered_id(), Some("msg2"));

        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 2);
        assert_eq!(state.get_hovered_id(), Some("msg3"));

        // Test navigation at bottom boundary
        state.edit_selection_next();
        assert_eq!(state.edit_selection_index, 2);
        assert_eq!(state.get_hovered_id(), Some("msg3"));
    }

    #[test]
    fn test_clear_edit_selection() {
        let mut state = InputPanelState {
            edit_selection_messages: vec![("msg1".to_string(), "First message".to_string())],
            edit_selection_index: 0,
            edit_selection_hovered_id: Some("msg1".to_string()),
            ..Default::default()
        };

        // Clear it
        state.clear_edit_selection();

        assert!(state.edit_selection_messages.is_empty());
        assert_eq!(state.edit_selection_index, 0);
        assert!(state.edit_selection_hovered_id.is_none());
    }

    #[test]
    fn test_required_height_calculation() {
        let mut state = InputPanelState::default();

        // Empty textarea
        assert_eq!(state.required_height(None, 80, 10), 4); // 1 line + 3 for borders/padding

        // Multi-line content
        state.set_content_from_lines(vec!["Line 1", "Line 2", "Line 3"]);
        assert_eq!(state.required_height(None, 80, 10), 6); // 3 lines + 3

        // Test max height constraint
        state.set_content_from_lines(vec!["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]);
        assert_eq!(state.required_height(None, 80, 8), 8); // Capped at max
    }

    #[test]
    fn test_populate_edit_selection() {
        let mut state = InputPanelState::default();

        // Create test chat items
        let chat_items = vec![
            ChatItem::Message(MessageRow::new(Message::User {
                id: "user1".to_string(),
                content: vec![UserContent::Text {
                    text: "First user message".to_string(),
                }],
                timestamp: 123,
                parent_message_id: None,
            })),
            ChatItem::Message(MessageRow::new(Message::Assistant {
                id: "assistant1".to_string(),
                content: vec![],
                timestamp: 124,
                parent_message_id: None,
            })),
            ChatItem::Message(MessageRow::new(Message::User {
                id: "user2".to_string(),
                content: vec![UserContent::Text {
                    text: "Second user message".to_string(),
                }],
                timestamp: 125,
                parent_message_id: None,
            })),
        ];

        state.populate_edit_selection(chat_items.iter());

        // Should have 2 user messages
        assert_eq!(state.edit_selection_messages.len(), 2);
        assert_eq!(state.edit_selection_messages[0].0, "user1");
        assert_eq!(state.edit_selection_messages[0].1, "First user message");
        assert_eq!(state.edit_selection_messages[1].0, "user2");
        assert_eq!(state.edit_selection_messages[1].1, "Second user message");

        // Should select the last message
        assert_eq!(state.edit_selection_index, 1);
        assert_eq!(state.get_hovered_id(), Some("user2"));
    }
}
