use async_trait::async_trait;
use eyre::{Result, eyre};
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use steer_core::tools::dispatch_agent::DISPATCH_AGENT_TOOL_NAME;
use steer_core::tools::fetch::FETCH_TOOL_NAME;
use steer_tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
    MULTI_EDIT_TOOL_NAME, REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME,
    VIEW_TOOL_NAME,
};

use super::Command;
use crate::session_config::{SessionConfigLoader, SessionConfigOverrides};
use steer_core::app::MessageData;
use steer_core::app::conversation::{Message, UserContent};

pub struct HeadlessCommand {
    pub model: Option<String>,
    pub messages_json: Option<PathBuf>,
    pub global_model: String,
    pub session: Option<String>,
    pub session_config: Option<PathBuf>,
    pub system_prompt: Option<String>,
    pub remote: Option<String>,
    pub directory: Option<PathBuf>,
    pub catalogs: Vec<PathBuf>,
}

#[async_trait]
impl Command for HeadlessCommand {
    async fn execute(&self) -> Result<()> {
        // Parse input into Vec<Message>
        let messages = if let Some(json_path) = &self.messages_json {
            // Read messages from JSON file
            let json_content = fs::read_to_string(json_path)
                .map_err(|e| eyre!("Failed to read messages JSON file: {}", e))?;

            serde_json::from_str::<Vec<Message>>(&json_content)
                .map_err(|e| eyre!("Failed to parse messages JSON: {}", e))?
        } else {
            // Read prompt from stdin
            let mut buffer = String::new();
            match io::stdin().read_to_string(&mut buffer) {
                Ok(_) => {
                    if buffer.trim().is_empty() {
                        return Err(eyre!("No input provided via stdin"));
                    }
                }
                Err(e) => return Err(eyre!("Failed to read from stdin: {}", e)),
            }
            // Create a single user message from stdin content
            vec![Message {
                data: MessageData::User {
                    content: vec![UserContent::Text { text: buffer }],
                },
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
                parent_message_id: None,
            }]
        };

        // Use model override if provided, otherwise use the global setting
        let model_to_use = self.model.as_ref().unwrap_or(&self.global_model);

        // Normalize provided catalog paths (if any)
        let normalized_catalogs: Vec<String> = self
            .catalogs
            .iter()
            .map(|p| {
                if !p.exists() {
                    tracing::warn!("Catalog path does not exist: {}", p.display());
                    p.to_string_lossy().to_string()
                } else {
                    p.canonicalize()
                        .map(|c| c.to_string_lossy().to_string())
                        .unwrap_or_else(|_| p.to_string_lossy().to_string())
                }
            })
            .collect();

        // Load session configuration (explicit path if provided, else auto-discovery)
        let session_config = if let Some(config_path) = &self.session_config {
            let overrides = SessionConfigOverrides {
                system_prompt: self.system_prompt.clone(),
                ..Default::default()
            };

            let loader =
                SessionConfigLoader::new(Some(config_path.clone())).with_overrides(overrides);

            Some(loader.load().await?)
        } else {
            let overrides = SessionConfigOverrides {
                system_prompt: self.system_prompt.clone(),
                ..Default::default()
            };
            let loader = SessionConfigLoader::new(None).with_overrides(overrides);
            Some(loader.load().await?)
        };

        // Extract tool config and system prompt from session config if available
        let (tool_config, system_prompt_to_use) = match &session_config {
            Some(config) => (
                Some(config.tool_config.clone()),
                config.system_prompt.clone().or(self.system_prompt.clone()),
            ),
            None => (None, self.system_prompt.clone()),
        };

        // Create session manager with custom catalogs (discovered catalogs are added by core loaders)
        let session_manager = crate::create_session_manager_with_catalog(
            model_to_use.clone(),
            normalized_catalogs.clone(),
        )
        .await?;

        // Determine execution mode and run
        let result = match &self.session {
            Some(session_id) => {
                // Run in existing session
                if messages.len() != 1 {
                    return Err(eyre!(
                        "When using --session, only single message input is supported (use stdin, not --messages-json)"
                    ));
                }

                let message = match &messages[0].data {
                    MessageData::User { content, .. } => {
                        // Extract text from the first UserContent block
                        match content.first() {
                            Some(UserContent::Text { text }) => text.clone(),
                            _ => {
                                return Err(eyre!(
                                    "Only text messages are supported when using --session"
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(eyre!(
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
                let app_messages: Result<Vec<Message>, _> =
                    messages.into_iter().map(Message::try_from).collect();

                let app_messages =
                    app_messages.map_err(|e| eyre!("Failed to convert messages: {}", e))?;

                crate::run_once_ephemeral_with_catalog(
                    &session_manager,
                    app_messages,
                    model_to_use.clone(),
                    tool_config,
                    Some(auto_approve_policy),
                    system_prompt_to_use,
                    normalized_catalogs,
                )
                .await?
            }
        };

        // Output the result as JSON
        let json_output = serde_json::to_string_pretty(&result)
            .map_err(|e| eyre!("Failed to serialize result to JSON: {}", e))?;

        println!("{json_output}");
        Ok(())
    }
}
