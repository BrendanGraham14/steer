use anyhow::{Result, anyhow};
use async_trait::async_trait;
use conductor_core::tools::dispatch_agent::DISPATCH_AGENT_TOOL_NAME;
use conductor_core::tools::fetch::FETCH_TOOL_NAME;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
    MULTI_EDIT_TOOL_NAME, REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME,
    VIEW_TOOL_NAME,
};

use super::Command;
use conductor_core::api::Model;
use conductor_core::app::conversation::{Message, UserContent};
use conductor_core::session::SessionToolConfig;

pub struct HeadlessCommand {
    pub model: Option<Model>,
    pub messages_json: Option<PathBuf>,
    pub global_model: Model,
    pub session: Option<String>,
    pub tool_config: Option<PathBuf>,
    pub system_prompt: Option<String>,
    pub remote: Option<String>,
    pub directory: Option<PathBuf>,
}

#[async_trait]
impl Command for HeadlessCommand {
    async fn execute(&self) -> Result<()> {
        // Parse input into Vec<Message>
        let messages = if let Some(json_path) = &self.messages_json {
            // Read messages from JSON file
            let json_content = fs::read_to_string(json_path)
                .map_err(|e| anyhow!("Failed to read messages JSON file: {}", e))?;

            serde_json::from_str::<Vec<Message>>(&json_content)
                .map_err(|e| anyhow!("Failed to parse messages JSON: {}", e))?
        } else {
            // Read prompt from stdin
            let mut buffer = String::new();
            match io::stdin().read_to_string(&mut buffer) {
                Ok(_) => {
                    if buffer.trim().is_empty() {
                        return Err(anyhow!("No input provided via stdin"));
                    }
                }
                Err(e) => return Err(anyhow!("Failed to read from stdin: {}", e)),
            }
            // Create a single user message from stdin content
            vec![Message::User {
                content: vec![UserContent::Text { text: buffer }],
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
            }]
        };

        // Use model override if provided, otherwise use the global setting
        let model_to_use = self.model.unwrap_or(self.global_model);

        // Load tool configuration if provided
        let tool_config = if let Some(config_path) = &self.tool_config {
            let config_content = fs::read_to_string(config_path)
                .map_err(|e| anyhow!("Failed to read tool config file: {}", e))?;

            let config: SessionToolConfig = serde_json::from_str(&config_content)
                .map_err(|e| anyhow!("Failed to parse tool config JSON: {}", e))?;

            Some(config)
        } else {
            None
        };

        // Create session manager
        let session_manager = crate::create_session_manager().await?;

        // Determine execution mode and run
        let result = match &self.session {
            Some(session_id) => {
                // Run in existing session
                if messages.len() != 1 {
                    return Err(anyhow!(
                        "When using --session, only single message input is supported (use stdin, not --messages-json)"
                    ));
                }

                let message = match &messages[0] {
                    Message::User { content, .. } => {
                        // Extract text from the first UserContent block
                        match content.first() {
                            Some(UserContent::Text { text }) => text.clone(),
                            _ => {
                                return Err(anyhow!(
                                    "Only text messages are supported when using --session"
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(anyhow!(
                            "Only user messages are supported when using --session"
                        ));
                    }
                };

                crate::run_once_in_session(&session_manager, session_id.clone(), message).await?
            }
            _ => {
                // Run in new ephemeral session (default behavior)
                // For headless mode, auto-approve all tools for convenience
                let auto_approve_policy = {
                    let all_tools = [
                        BASH_TOOL_NAME,
                        GREP_TOOL_NAME,
                        GLOB_TOOL_NAME,
                        LS_TOOL_NAME,
                        VIEW_TOOL_NAME,
                        EDIT_TOOL_NAME,
                        MULTI_EDIT_TOOL_NAME,
                        REPLACE_TOOL_NAME,
                        TODO_READ_TOOL_NAME,
                        TODO_WRITE_TOOL_NAME,
                        FETCH_TOOL_NAME,
                        DISPATCH_AGENT_TOOL_NAME,
                    ]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<std::collections::HashSet<String>>();
                    crate::session::ToolApprovalPolicy::PreApproved { tools: all_tools }
                };

                // Convert API messages to app messages
                let app_messages: Result<Vec<crate::app::Message>, _> = messages
                    .into_iter()
                    .map(crate::app::Message::try_from)
                    .collect();

                let app_messages =
                    app_messages.map_err(|e| anyhow!("Failed to convert messages: {}", e))?;

                crate::run_once_ephemeral(
                    &session_manager,
                    app_messages,
                    model_to_use,
                    tool_config,
                    Some(auto_approve_policy),
                    self.system_prompt.clone(),
                )
                .await?
            }
        };

        // Output the result as JSON
        let json_output = serde_json::to_string_pretty(&result)
            .map_err(|e| anyhow!("Failed to serialize result to JSON: {}", e))?;

        println!("{}", json_output);
        Ok(())
    }
}
