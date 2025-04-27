use anyhow::Result;
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

/// Command filter for enhanced security
pub struct CommandFilter {
    /// API key for Claude API
    api_key: String,
    /// Set of allowed command prefixes
    allowed_prefixes: HashSet<String>,
}

impl CommandFilter {
    /// Create a new command filter
    pub fn new(api_key: &str) -> Self {
        // Default set of safe commands that are always allowed
        let mut allowed_prefixes = HashSet::new();
        allowed_prefixes.insert("ls".to_string());
        allowed_prefixes.insert("pwd".to_string());
        allowed_prefixes.insert("cd".to_string());
        allowed_prefixes.insert("echo".to_string());
        allowed_prefixes.insert("mkdir".to_string());
        allowed_prefixes.insert("cat".to_string());
        allowed_prefixes.insert("less".to_string());
        allowed_prefixes.insert("grep".to_string());
        allowed_prefixes.insert("find".to_string());
        allowed_prefixes.insert("head".to_string());
        allowed_prefixes.insert("tail".to_string());
        allowed_prefixes.insert("diff".to_string());
        allowed_prefixes.insert("cp".to_string());
        allowed_prefixes.insert("mv".to_string());
        allowed_prefixes.insert("rm".to_string());
        allowed_prefixes.insert("rmdir".to_string());
        allowed_prefixes.insert("touch".to_string());
        allowed_prefixes.insert("git status".to_string());
        allowed_prefixes.insert("git diff".to_string());
        allowed_prefixes.insert("git log".to_string());
        allowed_prefixes.insert("git show".to_string());
        allowed_prefixes.insert("git branch".to_string());
        allowed_prefixes.insert("cargo build".to_string());
        allowed_prefixes.insert("cargo run".to_string());
        allowed_prefixes.insert("cargo test".to_string());
        allowed_prefixes.insert("cargo check".to_string());
        allowed_prefixes.insert("cargo clippy".to_string());
        allowed_prefixes.insert("cargo fmt".to_string());
        allowed_prefixes.insert("cargo doc".to_string());

        Self {
            api_key: api_key.to_string(),
            allowed_prefixes,
        }
    }

    /// Add an allowed command prefix
    pub fn add_allowed_prefix(&mut self, prefix: &str) {
        self.allowed_prefixes.insert(prefix.to_string());
    }

    /// Remove an allowed command prefix
    pub fn remove_allowed_prefix(&mut self, prefix: &str) {
        self.allowed_prefixes.remove(prefix);
    }

    /// Get the list of allowed command prefixes
    pub fn allowed_prefixes(&self) -> Vec<String> {
        self.allowed_prefixes.iter().cloned().collect()
    }

    /// Check if a command is allowed to run
    pub async fn is_command_allowed(
        &self,
        command: &str,
        token: CancellationToken,
    ) -> Result<bool> {
        // Get the command prefix
        let prefix = self.get_command_prefix(command, token).await?;

        // Check if the prefix is "none" (no prefix)
        if prefix == "none" {
            // Commands with no prefix are allowed
            return Ok(true);
        }

        // Check if the prefix is detected as a command injection
        if prefix == "command_injection_detected" {
            // Command injection is not allowed
            return Ok(false);
        }

        // Check if the prefix is in the allowed list
        Ok(self.allowed_prefixes.contains(&prefix))
    }

    /// Get the prefix of a command
    async fn get_command_prefix(&self, command: &str, token: CancellationToken) -> Result<String> {
        // Read the command filter prompt
        let prompt_template = include_str!("../../prompts/command_filter.md");

        // Create the client
        let client = crate::api::Client::new(&self.api_key);

        // Replace the placeholder with the actual command
        let user_message = prompt_template.replace("${command}", command);

        // Create the messages
        let messages = vec![
            crate::api::Message {
                role: "system".to_string(),
                content: crate::api::messages::MessageContent::Text {
                    content: "Your task is to process Bash commands that an AI coding agent wants to run.".to_string(),
                },
                id: None,
            },
            crate::api::Message {
                role: "user".to_string(),
                content: crate::api::messages::MessageContent::Text {
                    content: user_message,
                },
                id: None,
            },
        ];

        // Don't create a new token here, use the passed one
        // let token = CancellationToken::new();

        // Call the API using the stored api_client
        match client
            .complete(messages, None, None, token) // Pass the token
            .await
        {
            Ok(response) => {
                // Extract the response text
                let prefix = response.extract_text();

                // Trim whitespace and return
                Ok(prefix.trim().to_string())
            }
            Err(e) => {
                // Handle the error
                Err(e.into())
            }
        }
    }
}
