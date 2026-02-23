use async_trait::async_trait;
use eyre::{Result, eyre};
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use steer_core::app::domain::types::SessionId;
use steer_core::tools::{DISPATCH_AGENT_TOOL_NAME, FETCH_TOOL_NAME};
use steer_tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
    MULTI_EDIT_TOOL_NAME, REPLACE_TOOL_NAME, TODO_READ_TOOL_NAME, TODO_WRITE_TOOL_NAME,
    VIEW_TOOL_NAME,
};

use super::Command;
use crate::session_config::{SessionConfigLoader, SessionConfigOverrides};
use steer_core::app::MessageData;
use steer_core::app::conversation::{Message, UserContent};
use steer_core::session::{ApprovalRulesOverrides, ToolApprovalPolicyOverrides};

pub struct HeadlessCommand {
    pub model: Option<String>,
    pub messages_json: Option<PathBuf>,
    pub global_model: String,
    pub session: Option<String>,
    pub session_config: Option<PathBuf>,
    pub remote: Option<String>,
    pub directory: Option<PathBuf>,
    pub catalogs: Vec<PathBuf>,
}

#[async_trait]
impl Command for HeadlessCommand {
    async fn execute(&self) -> Result<()> {

        let message = self.extract_message()?;
        let model_to_use = self.model.as_ref().unwrap_or(&self.global_model);
        let normalized_catalogs = self.normalize_catalog_paths();

        let (runtime, model) =
            crate::create_runtime_with_catalogs(model_to_use.clone(), normalized_catalogs.clone())
                .await?;

        let result = if let Some(session_id_str) = &self.session {
            let session_id = SessionId::parse(session_id_str)
                .ok_or_else(|| eyre!("Invalid session ID: {}", session_id_str))?;

            crate::run_once_in_session(&runtime.handle, session_id, message, model).await?
        } else {
            let session_config = self.build_session_config(model.clone()).await?;
            crate::run_once_new_session(&runtime.handle, session_config, message, model).await?
        };

        runtime.shutdown().await;

        let json_output = serde_json::to_string_pretty(&result)
            .map_err(|e| eyre!("Failed to serialize result to JSON: {}", e))?;

        let mut stdout = io::stdout();
        writeln!(stdout, "{json_output}")?;
        Ok(())
    }
}

impl HeadlessCommand {
    fn extract_message(&self) -> Result<String> {
        if let Some(json_path) = &self.messages_json {
            let json_content = fs::read_to_string(json_path)
                .map_err(|e| eyre!("Failed to read messages JSON file: {}", e))?;

            let messages: Vec<Message> = serde_json::from_str(&json_content)
                .map_err(|e| eyre!("Failed to parse messages JSON: {}", e))?;

            let last_message = messages
                .last()
                .ok_or_else(|| eyre!("No messages in JSON file"))?;

            match &last_message.data {
                MessageData::User { content, .. } => content
                    .iter()
                    .find_map(|c| match c {
                        UserContent::Text { text } => Some(text.clone()),
                        UserContent::Image { .. } | UserContent::CommandExecution { .. } => None,
                    })
                    .ok_or_else(|| eyre!("Last message must contain text content")),
                _ => Err(eyre!("Last message must be from User")),
            }
        } else {
            let mut buffer = String::new();
            io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|e| eyre!("Failed to read from stdin: {}", e))?;

            if buffer.trim().is_empty() {
                return Err(eyre!("No input provided via stdin"));
            }

            Ok(buffer)
        }
    }

    fn normalize_catalog_paths(&self) -> Vec<String> {
        self.catalogs
            .iter()
            .map(|p| {
                if p.exists() {
                    p.canonicalize().map_or_else(
                        |_| p.to_string_lossy().to_string(),
                        |c| c.to_string_lossy().to_string(),
                    )
                } else {
                    tracing::warn!("Catalog path does not exist: {}", p.display());
                    p.to_string_lossy().to_string()
                }
            })
            .collect()
    }

    async fn build_session_config(
        &self,
        default_model: steer_core::config::model::ModelId,
    ) -> Result<steer_core::session::state::SessionConfig> {
        let overrides = SessionConfigOverrides {
            default_model: self.model.as_ref().map(|_| default_model.clone()),
            ..Default::default()
        };

        let loader = if let Some(config_path) = &self.session_config {
            SessionConfigLoader::new(default_model, Some(config_path.clone()))
                .with_overrides(overrides)
        } else {
            SessionConfigLoader::new(default_model, None).with_overrides(overrides)
        };

        let mut config = loader.load().await?;

        let auto_approve_rules = {
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
            .map(|s| (*s).to_string())
            .collect::<std::collections::HashSet<String>>();
            ApprovalRulesOverrides {
                tools: all_tools,
                per_tool: std::collections::HashMap::new(),
            }
        };

        config.policy_overrides.approval_policy = ToolApprovalPolicyOverrides {
            preapproved: auto_approve_rules,
        };
        config
            .metadata
            .insert("mode".to_string(), "headless".to_string());

        Ok(config)
    }
}
