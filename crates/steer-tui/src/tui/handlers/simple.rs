use crate::error::Result;
use crate::tui::InputMode;
use crate::tui::NoticeLevel;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;
use tui_textarea::Input;

impl Tui {
    pub async fn handle_simple_mode(&mut self, key: KeyEvent) -> Result<bool> {
        // Check for special modes first
        match self.input_mode {
            InputMode::BashCommand => return self.handle_bash_mode(key).await,
            InputMode::AwaitingApproval => return self.handle_approval_mode(key).await,
            InputMode::EditMessageSelection => return self.handle_edit_selection_mode(key).await,
            InputMode::FuzzyFinder => return self.handle_fuzzy_finder_mode(key).await,
            InputMode::ConfirmExit => return self.handle_confirm_exit_mode(key).await,
            InputMode::Setup => return self.handle_setup_mode(key).await,
            InputMode::Simple | InputMode::VimInsert | InputMode::VimNormal => {}
        }

        match key.code {
            KeyCode::BackTab => {
                self.cycle_primary_agent().await;
            }

            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.cycle_primary_agent().await;
            }

            KeyCode::Esc => {
                // Check for double-tap first
                if self.editing_message_id.is_some() {
                    self.cancel_edit_mode();
                    self.double_tap_tracker.clear_key(&KeyCode::Esc);
                    return Ok(false);
                }

                if self
                    .double_tap_tracker
                    .is_double_tap(KeyCode::Esc, Duration::from_millis(300))
                {
                    // Double ESC
                    let content = self.input_panel_state.content();
                    if content.is_empty() {
                        // Empty content - edit previous message
                        self.enter_edit_selection_mode();
                    } else {
                        // Has content - clear it
                        self.input_panel_state.clear();
                        self.pending_attachments.clear();
                    }
                    // Clear to prevent triple-tap
                    self.double_tap_tracker.clear_key(&KeyCode::Esc);
                } else {
                    // Single ESC. If this cancels an in-flight op with queued work,
                    // do not count it toward double-tap so restored input isn't cleared.
                    let canceling_with_queued_item = self.is_processing && self.queued_count > 0;
                    if canceling_with_queued_item {
                        self.double_tap_tracker.clear_key(&KeyCode::Esc);
                    } else {
                        self.double_tap_tracker.record_key(KeyCode::Esc);
                    }

                    if self.is_processing {
                        self.client.cancel_operation().await?;
                    }
                    // Don't trigger confirm exit - that's only for Ctrl+C/Ctrl+D
                }
            }

            // Multi-line support - handle before regular Enter
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT)
                    || key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.input_panel_state
                    .handle_input(Input::from(KeyEvent::new(
                        KeyCode::Char('\n'),
                        KeyModifiers::empty(),
                    )));
                self.sync_attachments_from_input_tokens();
            }

            KeyCode::Enter => {
                let content = self.input_panel_state.content().trim().to_string();
                if self.has_pending_send_content() {
                    if !self.pending_attachments.is_empty() && content.starts_with('/') {
                        self.push_notice(
                            NoticeLevel::Warn,
                            "Image attachments are only supported for regular prompts.".to_string(),
                        );
                        return Ok(false);
                    }
                    if content.starts_with('!') && content.len() > 1 {
                        // Execute as bash command
                        let command = content[1..].trim().to_string();
                        self.client.execute_bash_command(command).await?;
                    } else if content.starts_with('/') {
                        // Handle as slash command
                        self.handle_slash_command(content).await?;
                    } else {
                        // Send as normal message
                        self.send_message(content).await?;
                    }
                    self.input_panel_state.clear();
                    self.pending_attachments.clear();
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
                    self.sync_attachments_from_input_tokens();
                }
            }

            KeyCode::Char('/') => {
                let content = self.input_panel_state.content();
                if content.is_empty() {
                    // First character - activate command fuzzy finder
                    self.input_panel_state.handle_input(Input::from(key));
                    self.input_panel_state.activate_command_fuzzy();
                    self.switch_mode(InputMode::FuzzyFinder);

                    // Immediately show all commands
                    let results: Vec<_> = self
                        .command_registry
                        .all_commands()
                        .into_iter()
                        .map(|cmd| {
                            crate::tui::widgets::fuzzy_finder::PickerItem::new(
                                cmd.name.clone(),
                                format!("/{} ", cmd.name),
                            )
                        })
                        .collect();
                    self.input_panel_state.fuzzy_finder.update_results(results);
                } else {
                    // Normal / character
                    self.input_panel_state.handle_input(Input::from(key));
                    self.sync_attachments_from_input_tokens();
                }
            }

            KeyCode::Char('@') => {
                // Always activate file fuzzy finder
                self.input_panel_state.handle_input(Input::from(key));
                self.sync_attachments_from_input_tokens();
                self.input_panel_state.activate_fuzzy();
                self.switch_mode(InputMode::FuzzyFinder);

                // Immediately show all files (limited to 20)
                let file_results = self
                    .input_panel_state
                    .file_cache()
                    .fuzzy_search("", Some(20))
                    .await;
                let picker_items: Vec<_> = file_results
                    .into_iter()
                    .map(|path| {
                        crate::tui::widgets::fuzzy_finder::PickerItem::new(
                            path.clone(),
                            format!("@{path} "),
                        )
                    })
                    .collect();
                self.input_panel_state
                    .fuzzy_finder
                    .update_results(picker_items);
            }

            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.is_processing {
                    self.client.cancel_operation().await?;
                } else {
                    self.switch_mode(InputMode::ConfirmExit);
                }
            }

            KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                if let Some(head) = &self.queued_head {
                    if let Err(e) = self.client.dequeue_queued_item().await {
                        self.push_notice(NoticeLevel::Error, Self::format_grpc_error(&e));
                        return Ok(false);
                    }

                    let content = match head.kind {
                        steer_grpc::client_api::QueuedWorkKind::DirectBash => {
                            format!("!{}", head.content)
                        }
                        _ => head.content.clone(),
                    };
                    self.input_panel_state.replace_content(&content, None);
                    self.sync_attachments_from_input_tokens();
                }
            }

            // Toggle view mode with Ctrl+R
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.chat_viewport.state_mut().toggle_view_mode();
                self.chat_viewport.state_mut().scroll_to_bottom();
            }
            _ => {
                // Try common text manipulation first
                if self.handle_text_manipulation(key)? {
                    return Ok(false);
                }

                // Normal text input
                self.input_panel_state.handle_input(Input::from(key));
                self.sync_attachments_from_input_tokens();

                // Reset placeholder if needed
                if self.input_panel_state.content().is_empty() {
                    self.input_panel_state
                        .textarea
                        .set_placeholder_text("Type your message here...");
                }
            }
        }

        Ok(false)
    }
}
