use std::fmt;

/// Identifies a tool that was active during cancellation
#[derive(Debug, Clone)]
pub struct ActiveTool {
    pub id: String,   // The unique tool call ID
    pub name: String, // The name of the tool (e.g., "Bash", "GlobTool")
}

impl fmt::Display for ActiveTool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (ID: {})", self.name, self.id)
    }
}

/// Represents the state of operations when cancellation occurred
#[derive(Debug, Clone)]
pub struct CancellationInfo {
    /// Whether there was an API call in progress
    pub api_call_in_progress: bool,

    /// Tools that were active at cancellation time
    pub active_tools: Vec<ActiveTool>,

    /// Whether there were tools pending approval
    pub pending_tool_approvals: bool,
}

impl fmt::Display for CancellationInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut activities = Vec::new();

        if self.api_call_in_progress {
            activities.push("API call to Claude".to_string());
        }

        if !self.active_tools.is_empty() {
            if self.active_tools.len() == 1 {
                activities.push(format!("tool execution: {}", self.active_tools[0]));
            } else {
                activities.push(format!(
                    "multiple tool executions ({} tools)",
                    self.active_tools.len()
                ));
            }
        }

        if self.pending_tool_approvals {
            activities.push("waiting for tool approval".to_string());
        }

        if activities.is_empty() {
            write!(f, "no active operations")
        } else {
            write!(f, "{}", activities.join(", "))
        }
    }
}
