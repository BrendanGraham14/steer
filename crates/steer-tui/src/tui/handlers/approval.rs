use crate::error::Error;
use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use steer_core::error::Error as CoreError;
use steer_grpc::client_api::ApprovalDecision;
use steer_tools::tools::BASH_TOOL_NAME;
use steer_tools::tools::bash::BashParams;
use tracing::debug;

impl Tui {
    pub async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if let Some((request_id, tool_call)) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.client
                        .approve_tool(request_id.to_string(), ApprovalDecision::Once)
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    debug!(target: "handle_approval_mode", "Approving tool call with request_id '{:?}' and name '{}'", request_id, tool_call.name);
                    if tool_call.name == BASH_TOOL_NAME {
                        debug!(target: "handle_approval_mode", "(Always) Approving bash command with request_id '{:?}'", request_id);
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
                        self.client
                            .approve_tool(
                                request_id.to_string(),
                                ApprovalDecision::AlwaysBashPattern(bash_params.command.clone()),
                            )
                            .await?;
                    } else {
                        self.client
                            .approve_tool(request_id.to_string(), ApprovalDecision::AlwaysTool)
                            .await?;
                    }
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    if tool_call.name == BASH_TOOL_NAME {
                        self.client
                            .approve_tool(request_id.to_string(), ApprovalDecision::AlwaysTool)
                            .await?;
                        self.input_mode = self.default_input_mode();
                    } else {
                        self.current_tool_approval = Some((request_id, tool_call));
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.client
                        .approve_tool(request_id.to_string(), ApprovalDecision::Deny)
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                _ => {
                    self.current_tool_approval = Some((request_id, tool_call));
                }
            }
        } else {
            self.input_mode = self.default_input_mode();
        }
        Ok(false)
    }
}
