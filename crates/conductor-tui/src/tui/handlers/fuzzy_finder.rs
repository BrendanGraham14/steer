use crate::error::Result;
use crate::tui::theme::ThemeLoader;
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
            match mode {
                FuzzyFinderMode::Models | FuzzyFinderMode::Themes => {
                    // For models and themes, always stay active until explicitly closed
                    true
                }
                FuzzyFinderMode::Files | FuzzyFinderMode::Commands => {
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
                    if let Some(trigger_pos) =
                        self.input_panel_state.fuzzy_finder.trigger_position()
                    {
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
                }
            }
        };

        if !cursor_after_trigger {
            // The command fuzzy finder closed because we typed whitespace.
            // If the user just finished typing a top-level command like "/model " or
            // "/theme ", immediately open the next-level fuzzy finder.
            let reopen_handled = if mode == FuzzyFinderMode::Commands
                && key.code == KeyCode::Char(' ')
                && key.modifiers == KeyModifiers::NONE
            {
                let content = self.input_panel_state.content();
                let cursor_pos = self.input_panel_state.get_cursor_byte_offset();
                if cursor_pos > 0 {
                    use crate::tui::commands::{CoreCommandType, TuiCommandType};
                    let before_space = &content[..cursor_pos - 1]; // exclude the space itself
                    let model_cmd = format!("/{}", CoreCommandType::Model.command_name());
                    let theme_cmd = format!("/{}", TuiCommandType::Theme.command_name());
                    let is_model_cmd = before_space.trim_end().ends_with(&model_cmd);
                    let is_theme_cmd = before_space.trim_end().ends_with(&theme_cmd);
                    if is_model_cmd || is_theme_cmd {
                        // Don't clear the textarea - keep the command visible
                        // The fuzzy finder will overlay on top of the existing text

                        use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode as FMode;
                        if is_model_cmd {
                            self.input_panel_state
                                .fuzzy_finder
                                .activate(cursor_pos, FMode::Models);
                            // Populate models
                            use conductor_core::api::Model;

                            let current_model = self.current_model;
                            let models: Vec<String> = Model::iter_recommended()
                                .map(|m| {
                                    let n = m.as_ref();
                                    if m == current_model {
                                        format!("{n} (current)")
                                    } else {
                                        n.to_string()
                                    }
                                })
                                .collect();
                            self.input_panel_state.fuzzy_finder.update_results(models);
                        } else {
                            self.input_panel_state
                                .fuzzy_finder
                                .activate(cursor_pos, FMode::Themes);
                            // Populate themes
                            let loader = ThemeLoader::new();
                            let themes = loader.list_themes();
                            self.input_panel_state.fuzzy_finder.update_results(themes);
                        }
                        self.input_mode = InputMode::FuzzyFinder;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            } else {
                false
            };

            if !reopen_handled {
                self.input_panel_state.deactivate_fuzzy();
                self.input_mode = InputMode::Insert;
            }
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
                            // Check if this is model or theme command
                            use crate::tui::commands::{CoreCommandType, TuiCommandType};
                            let model_cmd_name = CoreCommandType::Model.command_name();
                            let theme_cmd_name = TuiCommandType::Theme.command_name();

                            if selected == model_cmd_name || selected == theme_cmd_name {
                                // User selected model or theme - open the appropriate fuzzy finder
                                let content = format!("/{selected} ");
                                self.input_panel_state.clear();
                                self.input_panel_state
                                    .set_content_from_lines(vec![&content]);

                                // Set cursor at end
                                let cursor_pos = content.len();
                                self.input_panel_state
                                    .textarea
                                    .move_cursor(tui_textarea::CursorMove::End);

                                use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode as FMode;
                                if selected == model_cmd_name {
                                    self.input_panel_state
                                        .fuzzy_finder
                                        .activate(cursor_pos, FMode::Models);

                                    // Populate models
                                    use conductor_core::api::Model;

                                    let current_model = self.current_model;
                                    let models: Vec<String> = Model::iter_recommended()
                                        .map(|m| {
                                            let model_str = m.as_ref();
                                            if m == current_model {
                                                format!("{model_str} (current)")
                                            } else {
                                                model_str.to_string()
                                            }
                                        })
                                        .collect();
                                    self.input_panel_state.fuzzy_finder.update_results(models);
                                } else {
                                    self.input_panel_state
                                        .fuzzy_finder
                                        .activate(cursor_pos, FMode::Themes);

                                    // Populate themes
                                    let loader = ThemeLoader::new();
                                    let themes = loader.list_themes();
                                    self.input_panel_state.fuzzy_finder.update_results(themes);
                                }
                                // Stay in fuzzy finder mode
                                self.input_mode = InputMode::FuzzyFinder;
                            } else {
                                // Complete with command normally
                                self.input_panel_state.complete_command_fuzzy(&selected);
                                self.input_panel_state.deactivate_fuzzy();
                                self.input_mode = InputMode::Insert;
                            }
                        }
                        FuzzyFinderMode::Models => {
                            // Extract model name (remove " (current)" suffix if present)
                            let model_name = selected.trim_end_matches(" (current)");
                            // Send the model command using command_name()
                            use crate::tui::commands::CoreCommandType;
                            let command = format!(
                                "/{} {}",
                                CoreCommandType::Model.command_name(),
                                model_name
                            );
                            self.send_message(command).await?;
                            // Clear the input after sending
                            self.input_panel_state.clear();
                        }
                        FuzzyFinderMode::Themes => {
                            // Send the theme command using command_name()
                            use crate::tui::commands::TuiCommandType;
                            let command =
                                format!("/{} {}", TuiCommandType::Theme.command_name(), selected);
                            self.send_message(command).await?;
                            // Clear the input after sending
                            self.input_panel_state.clear();
                        }
                    }
                    if mode != FuzzyFinderMode::Commands {
                        self.input_panel_state.deactivate_fuzzy();
                        self.input_mode = InputMode::Insert;
                    }
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
        } else if mode == FuzzyFinderMode::Models || mode == FuzzyFinderMode::Themes {
            // For models and themes, use the typed content *after the command prefix* as search query
            use crate::tui::commands::{CoreCommandType, TuiCommandType};
            let raw_content = self.input_panel_state.content();
            let query = match mode {
                FuzzyFinderMode::Models => {
                    let prefix = format!("/{} ", CoreCommandType::Model.command_name());
                    raw_content
                        .strip_prefix(&prefix)
                        .unwrap_or(&raw_content)
                        .to_string()
                }
                FuzzyFinderMode::Themes => {
                    let prefix = format!("/{} ", TuiCommandType::Theme.command_name());
                    raw_content
                        .strip_prefix(&prefix)
                        .unwrap_or(&raw_content)
                        .to_string()
                }
                _ => unreachable!(),
            };

            match mode {
                FuzzyFinderMode::Models => {
                    // Filter models based on query
                    use conductor_core::api::Model;
                    use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
                    use strum::IntoEnumIterator;

                    let matcher = SkimMatcherV2::default();
                    let current_model = self.current_model;

                    let mut scored_models: Vec<(i64, String)> = Model::iter_recommended()
                        .filter_map(|m| {
                            let model_str = m.as_ref();
                            let display_str = if m == current_model {
                                format!("{model_str} (current)")
                            } else {
                                model_str.to_string()
                            };

                            let display_score = matcher.fuzzy_match(&display_str, &query);
                            let exact_alias_match = m
                                .aliases()
                                .iter()
                                .any(|alias| alias.eq_ignore_ascii_case(&query));

                            let alias_score = if exact_alias_match {
                                Some(1000) // High score for exact matches
                            } else {
                                m.aliases()
                                    .iter()
                                    .filter_map(|alias| matcher.fuzzy_match(alias, &query))
                                    .max()
                            };

                            // Take the maximum score
                            let best_score = match (display_score, alias_score) {
                                (Some(d), Some(a)) => Some(d.max(a)),
                                (Some(d), None) => Some(d),
                                (None, Some(a)) => Some(a),
                                (None, None) => None,
                            };

                            best_score.map(|score| (score, display_str))
                        })
                        .collect();

                    // Sort by score (highest first)
                    scored_models.sort_by(|a, b| b.0.cmp(&a.0));

                    let results: Vec<String> =
                        scored_models.into_iter().map(|(_, model)| model).collect();

                    self.input_panel_state.fuzzy_finder.update_results(results);
                }
                FuzzyFinderMode::Themes => {
                    // Filter themes based on query
                    use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

                    let loader = ThemeLoader::new();
                    let all_themes = loader.list_themes();

                    if query.is_empty() {
                        self.input_panel_state
                            .fuzzy_finder
                            .update_results(all_themes);
                    } else {
                        let matcher = SkimMatcherV2::default();
                        let mut scored_themes: Vec<(i64, String)> = all_themes
                            .into_iter()
                            .filter_map(|theme| {
                                matcher
                                    .fuzzy_match(&theme, &query)
                                    .map(|score| (score, theme))
                            })
                            .collect();

                        // Sort by score (highest first)
                        scored_themes.sort_by(|a, b| b.0.cmp(&a.0));

                        let results: Vec<String> =
                            scored_themes.into_iter().map(|(_, theme)| theme).collect();

                        self.input_panel_state.fuzzy_finder.update_results(results);
                    }
                }
                _ => {}
            }
        }

        Ok(false)
    }
}
