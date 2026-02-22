use crate::error::Error;
use crate::error::Result;
use crate::tui::Tui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use steer_grpc::client_api::ApprovalDecision;
use steer_tools::tools::BASH_TOOL_NAME;
use steer_tools::tools::bash::BashParams;
use tracing::debug;

fn is_cycle_agent_key(key: KeyEvent) -> bool {
    key.code == KeyCode::BackTab
        || (key.code == KeyCode::Tab && key.modifiers.contains(KeyModifiers::SHIFT))
}

impl Tui {
    pub async fn handle_approval_mode(&mut self, key: KeyEvent) -> Result<bool> {
        if is_cycle_agent_key(key) {
            self.cycle_primary_agent().await;
            return Ok(false);
        }

        if let Some((request_id, tool_call)) = self.current_tool_approval.take() {
            match key.code {
                KeyCode::Char('y' | 'Y') => {
                    self.client
                        .approve_tool(request_id.to_string(), ApprovalDecision::Once)
                        .await?;
                    self.input_mode = self.default_input_mode();
                }
                KeyCode::Char('a' | 'A') => {
                    debug!(target: "handle_approval_mode", "Approving tool call with request_id '{:?}' and name '{}'", request_id, tool_call.name);
                    if tool_call.name == BASH_TOOL_NAME {
                        debug!(target: "handle_approval_mode", "(Always) Approving bash command with request_id '{:?}'", request_id);
                        let bash_params: BashParams =
                            serde_json::from_value(tool_call.parameters.clone()).map_err(|e| {
                                Error::CommandProcessing(format!(
                                    "Invalid bash params for approval: {e}"
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
                KeyCode::Char('l' | 'L') => {
                    if tool_call.name == BASH_TOOL_NAME {
                        self.client
                            .approve_tool(request_id.to_string(), ApprovalDecision::AlwaysTool)
                            .await?;
                        self.input_mode = self.default_input_mode();
                    } else {
                        self.current_tool_approval = Some((request_id, tool_call));
                    }
                }
                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
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

#[cfg(test)]
mod tests {
    use super::is_cycle_agent_key;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn detects_shift_tab_cycle_key_variants() {
        assert!(is_cycle_agent_key(KeyEvent::new(
            KeyCode::BackTab,
            KeyModifiers::NONE,
        )));
        assert!(is_cycle_agent_key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::SHIFT,
        )));
        assert!(is_cycle_agent_key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::SHIFT | KeyModifiers::CONTROL,
        )));
    }

    #[test]
    fn ignores_non_cycle_keys() {
        assert!(!is_cycle_agent_key(KeyEvent::new(
            KeyCode::Tab,
            KeyModifiers::NONE,
        )));
        assert!(!is_cycle_agent_key(KeyEvent::new(
            KeyCode::Char('y'),
            KeyModifiers::NONE,
        )));
    }
}
