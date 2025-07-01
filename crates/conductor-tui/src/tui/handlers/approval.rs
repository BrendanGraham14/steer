use crate::tui::{InputMode, Tui};
use anyhow::Result;
use conductor_core::app::AppCommand;
use crossterm::event::{KeyCode, KeyEvent};

impl Tui {
    pub async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some(tool_call) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Approve once
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: true,
                            always: false,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    // Approve always
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: true,
                            always: true,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Reject
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approved: false,
                            always: false,
                        })
                        .await?;
                    self.input_mode = InputMode::Normal;
                }
                _ => {
                    // Put it back if not handled
                    self.current_tool_approval = Some(tool_call);
                }
            }
        } else {
            // No approval pending, return to normal
            self.input_mode = InputMode::Normal;
        }
        Ok(false)
    }
}
