use crate::error::Error;
use crate::error::Result;
use crate::tui::Tui;
use conductor_core::app::{AppCommand, command::ApprovalType};
use conductor_core::error::Error as CoreError;
use conductor_tools::ToolError;
use conductor_tools::tools::BASH_TOOL_NAME;
use conductor_tools::tools::bash::BashParams;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use tracing::debug;

impl Tui {
    pub async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some(tool_call) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Approve once
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approval: ApprovalType::Once,
                        })
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    debug!(target: "handle_approval_mode", "Approving tool call with ID '{}' and name '{}'", tool_call.id, tool_call.name);
                    if tool_call.name == BASH_TOOL_NAME {
                        debug!(target: "handle_approval_mode", "(Always) Approving bash command with ID '{}'", tool_call.id);
                        // For bash commands, 'A' approves this specific command pattern
                        let bash_params: BashParams =
                            serde_json::from_value(tool_call.parameters.clone()).map_err(|e| {
                                Error::Core(CoreError::Tool(ToolError::InvalidParams(
                                    "bash".to_string(),
                                    e.to_string(),
                                )))
                            })?;
                        // Approve with the bash pattern payload
                        self.command_sink
                            .send_command(AppCommand::HandleToolResponse {
                                id: tool_call.id,
                                approval: ApprovalType::AlwaysBashPattern(
                                    bash_params.command.clone(),
                                ),
                            })
                            .await?;
                    } else {
                        // For non-bash tools, 'A' approves always
                        self.command_sink
                            .send_command(AppCommand::HandleToolResponse {
                                id: tool_call.id,
                                approval: ApprovalType::AlwaysTool,
                            })
                            .await?;
                    }
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    if tool_call.name == BASH_TOOL_NAME {
                        self.command_sink
                            .send_command(AppCommand::HandleToolResponse {
                                id: tool_call.id,
                                approval: ApprovalType::AlwaysTool,
                            })
                            .await?;
                        self.input_mode = self.default_input_mode();
                    } else {
                        // Put it back if not a bash command
                        self.current_tool_approval = Some(tool_call);
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Reject
                    self.command_sink
                        .send_command(AppCommand::HandleToolResponse {
                            id: tool_call.id,
                            approval: ApprovalType::Denied,
                        })
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                _ => {
                    // Put it back if not handled
                    self.current_tool_approval = Some(tool_call);
                }
            }
        } else {
            // No approval pending, return to normal
            self.input_mode = self.default_input_mode();
        }
        Ok(false)
    }
}
