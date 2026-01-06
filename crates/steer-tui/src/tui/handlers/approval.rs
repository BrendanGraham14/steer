use crate::error::Error;
use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use steer_core::error::Error as CoreError;
use steer_grpc::client_api::{ApprovalDecision, ClientCommand};
use steer_tools::tools::BASH_TOOL_NAME;
use steer_tools::tools::bash::BashParams;
use tracing::debug;

impl Tui {
    pub async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some((request_id, tool_call)) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Approve once
                    self.client
                        .send(ClientCommand::ApproveToolCall {
                            request_id,
                            decision: ApprovalDecision::Once,
                        })
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    debug!(target: "handle_approval_mode", "Approving tool call with request_id '{:?}' and name '{}'", request_id, tool_call.name);
                    if tool_call.name == BASH_TOOL_NAME {
                        debug!(target: "handle_approval_mode", "(Always) Approving bash command with request_id '{:?}'", request_id);
                        // For bash commands, 'A' approves this specific command pattern
                        let bash_params: BashParams =
                            serde_json::from_value(tool_call.parameters.clone()).map_err(|e| {
                                Error::Core(CoreError::Tool(
                                    steer_tools::ToolError::InvalidParams(
                                        "bash".to_string(),
                                        e.to_string(),
                                    )
                                    .into(),
                                ))
                            })?;
                        // Approve with the bash pattern payload
                        self.client
                            .send(ClientCommand::ApproveToolCall {
                                request_id,
                                decision: ApprovalDecision::AlwaysBashPattern(
                                    bash_params.command.clone(),
                                ),
                            })
                            .await?;
                    } else {
                        // For non-bash tools, 'A' approves always
                        self.client
                            .send(ClientCommand::ApproveToolCall {
                                request_id,
                                decision: ApprovalDecision::AlwaysTool,
                            })
                            .await?;
                    }
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    if tool_call.name == BASH_TOOL_NAME {
                        self.client
                            .send(ClientCommand::ApproveToolCall {
                                request_id,
                                decision: ApprovalDecision::AlwaysTool,
                            })
                            .await?;
                        self.input_mode = self.default_input_mode();
                    } else {
                        // Put it back if not a bash command
                        self.current_tool_approval = Some((request_id, tool_call));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Reject
                    self.client
                        .send(ClientCommand::ApproveToolCall {
                            request_id,
                            decision: ApprovalDecision::Deny,
                        })
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                _ => {
                    // Put it back if not handled
                    self.current_tool_approval = Some((request_id, tool_call));
                }
            }
        } else {
            // No approval pending, return to normal
            self.input_mode = self.default_input_mode();
        }
        Ok(false)
    }
}
