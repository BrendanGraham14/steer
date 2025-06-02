use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::time::Duration;

use crate::api::{
    Model,
    messages::{Message, MessageContent, MessageRole},
};
use crate::config::LlmConfig;
use super::Command;

pub struct HeadlessCommand {
    pub model: Option<Model>,
    pub messages_json: Option<PathBuf>,
    pub timeout: Option<u64>,
    pub global_model: Model,
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
            vec![Message {
                role: MessageRole::User,
                content: MessageContent::Text { content: buffer },
                id: None,
            }]
        };

        // Set up timeout if provided
        let timeout_duration = self.timeout.map(|secs| Duration::from_secs(secs));

        // Use model override if provided, otherwise use the global setting
        let model_to_use = self.model.unwrap_or(self.global_model);

        let llm_config = LlmConfig::from_env()
            .expect("Failed to load LLM configuration from environment variables.");

        // Run the agent in one-shot mode
        let result =
            crate::run_once(messages, model_to_use, &llm_config, timeout_duration).await?;

        // Output the result as JSON
        let json_output = serde_json::to_string_pretty(&result)
            .map_err(|e| anyhow!("Failed to serialize result to JSON: {}", e))?;

        println!("{}", json_output);
        Ok(())
    }
}