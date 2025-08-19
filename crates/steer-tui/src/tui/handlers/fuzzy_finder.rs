use crate::error::Result;
use crate::tui::InputMode;
use crate::tui::Tui;
use crate::tui::theme::ThemeLoader;
use crate::tui::widgets::PickerItem;
use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use steer_core::config::provider::ProviderId;
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
                            // Populate models from server
                            if let Ok(models) = self.client.list_models(None).await {
                                let current_model = self.current_model.clone();
                                let picker_items: Vec<PickerItem> = models
                                    .into_iter()
                                    .map(|m| {
                                        let model_id =
                                            (ProviderId(m.provider_id.clone()), m.model_id.clone());
                                        let display_name = m.display_name;
                                        let prov = ProviderId(m.provider_id.clone()).storage_key();
                                        let display_full = format!("{prov}/{display_name}");
                                        let display = if model_id == current_model {
                                            format!("{display_full} (current)")
                                        } else {
                                            display_full.clone()
                                        };
                                        // Insert provider/id for lookup
                                        let insert = format!("{}/{}", prov, m.model_id);
                                        PickerItem::new(display, insert)
                                    })
                                    .collect();
                                self.input_panel_state
                                    .fuzzy_finder
                                    .update_results(picker_items);
                            }
                        } else {
                            self.input_panel_state
                                .fuzzy_finder
                                .activate(cursor_pos, FMode::Themes);
                            // Populate themes
                            let loader = ThemeLoader::new();
                            let themes: Vec<_> = loader
                                .list_themes()
                                .into_iter()
                                .map(crate::tui::widgets::fuzzy_finder::PickerItem::simple)
                                .collect();
                            self.input_panel_state.fuzzy_finder.update_results(themes);
                        }
                        self.switch_mode(InputMode::FuzzyFinder);
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
                self.restore_previous_mode();
            }
            return Ok(false);
        }

        // Otherwise handle explicit results (Enter / Esc etc.)
        if let Some(result) = post_result {
            match result {
                FuzzyFinderResult::Close => {
                    self.input_panel_state.deactivate_fuzzy();
                    self.restore_previous_mode();
                }
                FuzzyFinderResult::Select(selected_item) => {
                    match mode {
                        FuzzyFinderMode::Files => {
                            // Complete with file path using the insert text
                            self.input_panel_state.complete_picker_item(&selected_item);
                        }
                        FuzzyFinderMode::Commands => {
                            // Extract just the command name from the label
                            let selected_cmd = selected_item.label.as_str();
                            // Check if this is model or theme command
                            use crate::tui::commands::{CoreCommandType, TuiCommandType};
                            let model_cmd_name = CoreCommandType::Model.command_name();
                            let theme_cmd_name = TuiCommandType::Theme.command_name();

                            if selected_cmd == model_cmd_name || selected_cmd == theme_cmd_name {
                                // User selected model or theme - open the appropriate fuzzy finder
                                let content = format!("/{selected_cmd} ");
                                self.input_panel_state.clear();
                                self.input_panel_state
                                    .set_content_from_lines(vec![&content]);

                                // Set cursor at end
                                let cursor_pos = content.len();
                                self.input_panel_state
                                    .textarea
                                    .move_cursor(tui_textarea::CursorMove::End);

                                use crate::tui::widgets::fuzzy_finder::FuzzyFinderMode as FMode;
                                if selected_cmd == model_cmd_name {
                                    self.input_panel_state
                                        .fuzzy_finder
                                        .activate(cursor_pos, FMode::Models);

                                    // Populate models from server
                                    if let Ok(models) = self.client.list_models(None).await {
                                        let current_model = self.current_model.clone();
                                        let results: Vec<PickerItem> = models
                                            .into_iter()
                                            .map(|m| {
                                                let model_id = (
                                                    ProviderId(m.provider_id.clone()),
                                                    m.model_id.clone(),
                                                );
                                                let prov =
                                                    ProviderId(m.provider_id.clone()).storage_key();
                                                let display_full =
                                                    format!("{}/{}", prov, m.display_name);
                                                let label = if model_id == current_model {
                                                    format!("{display_full} (current)")
                                                } else {
                                                    display_full.clone()
                                                };
                                                // Insert provider/id for lookup
                                                let insert = format!("{}/{}", prov, m.model_id);
                                                PickerItem::new(label, insert)
                                            })
                                            .collect();
                                        self.input_panel_state.fuzzy_finder.update_results(results);
                                    }
                                } else {
                                    self.input_panel_state
                                        .fuzzy_finder
                                        .activate(cursor_pos, FMode::Themes);

                                    // Populate themes
                                    let loader = ThemeLoader::new();
                                    let themes: Vec<_> = loader
                                        .list_themes()
                                        .into_iter()
                                        .map(crate::tui::widgets::fuzzy_finder::PickerItem::simple)
                                        .collect();
                                    self.input_panel_state.fuzzy_finder.update_results(themes);
                                }
                                // Stay in fuzzy finder mode
                                self.input_mode = InputMode::FuzzyFinder;
                            } else {
                                // Complete with command using the insert text
                                self.input_panel_state.complete_picker_item(&selected_item);
                                self.input_panel_state.deactivate_fuzzy();
                                self.restore_previous_mode();
                            }
                        }
                        FuzzyFinderMode::Models => {
                            // Use the insert text (provider/model_id) for command
                            use crate::tui::commands::CoreCommandType;
                            let command = format!(
                                "/{} {}",
                                CoreCommandType::Model.command_name(),
                                selected_item.insert
                            );
                            self.send_message(command).await?;
                            // Clear the input after sending
                            self.input_panel_state.clear();
                        }
                        FuzzyFinderMode::Themes => {
                            // Send the theme command using command_name()
                            use crate::tui::commands::TuiCommandType;
                            let command = format!(
                                "/{} {}",
                                TuiCommandType::Theme.command_name(),
                                selected_item.label
                            );
                            self.send_message(command).await?;
                            // Clear the input after sending
                            self.input_panel_state.clear();
                        }
                    }
                    if mode != FuzzyFinderMode::Commands {
                        self.input_panel_state.deactivate_fuzzy();
                        self.restore_previous_mode();
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
                    let results: Vec<_> = self
                        .command_registry
                        .search(query)
                        .into_iter()
                        .map(|cmd| {
                            crate::tui::widgets::fuzzy_finder::PickerItem::new(
                                cmd.name.to_string(),
                                format!("/{} ", cmd.name),
                            )
                        })
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
                    // Filter models based on query from server models
                    use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

                    if let Ok(models) = self.client.list_models(None).await {
                        let matcher = SkimMatcherV2::default();
                        let current_model = self.current_model.clone();
                        let mut scored_models: Vec<(i64, String, String)> = Vec::new();

                        for m in models {
                            let model_id = (ProviderId(m.provider_id.clone()), m.model_id.clone());
                            let prov = ProviderId(m.provider_id.clone()).storage_key();
                            let full_label = if model_id == current_model {
                                format!("{}/{} (current)", prov, m.display_name)
                            } else {
                                format!("{}/{}", prov, m.display_name)
                            };
                            // Insert provider/id for command completion
                            let insert = format!("{}/{}", prov, m.model_id);

                            // Match against full label, display name, model id, and aliases (alias and provider/alias)
                            let full_score = matcher.fuzzy_match(&full_label, &query);
                            let name_score = matcher.fuzzy_match(&m.display_name, &query);
                            let id_score = matcher.fuzzy_match(&m.model_id, &query);
                            let alias_score: Option<i64> = m
                                .aliases
                                .iter()
                                .filter_map(|a| {
                                    let s1 = matcher.fuzzy_match(a, &query);
                                    let s2 = matcher.fuzzy_match(&format!("{prov}/{a}"), &query);
                                    match (s1, s2) {
                                        (Some(x), Some(y)) => Some(x.max(y)),
                                        (Some(x), None) => Some(x),
                                        (None, Some(y)) => Some(y),
                                        (None, None) => None,
                                    }
                                })
                                .max();

                            // Take the maximum score from all matches
                            let best_score = [full_score, name_score, id_score, alias_score]
                                .into_iter()
                                .flatten()
                                .max();

                            if let Some(score) = best_score {
                                scored_models.push((score, full_label, insert));
                            }
                        }

                        // Sort by score (highest first)
                        scored_models.sort_by(|a, b| b.0.cmp(&a.0));

                        let results: Vec<_> = scored_models
                            .into_iter()
                            .map(|(_, label, insert)| {
                                crate::tui::widgets::fuzzy_finder::PickerItem::new(label, insert)
                            })
                            .collect();

                        self.input_panel_state.fuzzy_finder.update_results(results);
                    }
                }
                FuzzyFinderMode::Themes => {
                    // Filter themes based on query
                    use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};

                    let loader = ThemeLoader::new();
                    let all_themes = loader.list_themes();

                    if query.is_empty() {
                        let picker_items: Vec<_> = all_themes
                            .into_iter()
                            .map(crate::tui::widgets::fuzzy_finder::PickerItem::simple)
                            .collect();
                        self.input_panel_state
                            .fuzzy_finder
                            .update_results(picker_items);
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

                        let results: Vec<_> = scored_themes
                            .into_iter()
                            .map(|(_, theme)| {
                                crate::tui::widgets::fuzzy_finder::PickerItem::simple(theme)
                            })
                            .collect();

                        self.input_panel_state.fuzzy_finder.update_results(results);
                    }
                }
                _ => {}
            }
        }

        Ok(false)
    }
}
