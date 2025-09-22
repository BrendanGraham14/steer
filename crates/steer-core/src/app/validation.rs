use crate::config::LlmConfigProvider;
use async_trait::async_trait;
use std::collections::HashMap;
use steer_tools::ToolCall;
use steer_tools::tools::bash::BASH_TOOL_NAME;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct ValidationContext {
    pub cancellation_token: CancellationToken,
    pub llm_config_provider: LlmConfigProvider,
}

#[derive(Debug)]
pub struct ValidationResult {
    pub allowed: bool,
    pub reason: Option<String>,
    pub requires_user_approval: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Invalid parameters: {0}")]
    InvalidParams(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Other error: {0}")]
    Other(String),
}

#[async_trait]
pub trait ToolValidator: Send + Sync {
    fn tool_name(&self) -> &'static str;

    async fn validate(
        &self,
        tool_call: &ToolCall,
        context: &ValidationContext,
    ) -> Result<ValidationResult, ValidationError>;
}

pub struct ValidatorRegistry {
    validators: HashMap<String, Box<dyn ToolValidator>>,
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ValidatorRegistry {
    pub fn new() -> Self {
        let mut validators = HashMap::new();

        // Register tool-specific validators
        validators.insert(
            BASH_TOOL_NAME.to_string(),
            Box::new(BashValidator::new()) as Box<dyn ToolValidator>,
        );

        Self { validators }
    }

    pub fn get_validator(&self, tool_name: &str) -> Option<&dyn ToolValidator> {
        self.validators.get(tool_name).map(|v| v.as_ref())
    }
}

// Bash validator implementation
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BashParams {
    pub command: String,
    pub timeout: Option<u64>,
}

pub struct BashValidator;

impl Default for BashValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl BashValidator {
    pub fn new() -> Self {
        Self
    }

    /// Check if a command is banned (basic, fast check) - matches src/tools/bash.rs
    fn is_banned_command(&self, command: &str) -> bool {
        static BANNED_COMMAND_REGEXES: Lazy<Vec<Regex>> = Lazy::new(|| {
            let banned_commands = [
                // Network tools
                "curl",
                "wget",
                "nc",
                "telnet",
                "ssh",
                "scp",
                "ftp",
                "sftp",
                // Web browsers/clients
                "lynx",
                "w3m",
                "links",
                "elinks",
                "httpie",
                "xh",
                "http-prompt",
                "chrome",
                "firefox",
                "safari",
                "edge",
                "opera",
                "chromium",
                // Download managers
                "axel",
                "aria2c",
                // Shell utilities that might be risky if misused
                "alias",
                "unalias",
                "exec",
                "source",
                ".",
                "history",
                // Potentially dangerous system modification tools
                "sudo",
                "su",
                "chown",
                "chmod",
                "useradd",
                "userdel",
                "groupadd",
                "groupdel",
                // File editors (could be used to modify sensitive files)
                "vi",
                "vim",
                "nano",
                "pico",
                "emacs",
                "ed",
            ];
            banned_commands
                .iter()
                .map(|cmd| {
                    Regex::new(&format!(r"^\s*(\S*/)?{}\b", regex::escape(cmd)))
                        .expect("Failed to compile banned command regex")
                })
                .collect()
        });

        BANNED_COMMAND_REGEXES.iter().any(|re| re.is_match(command))
    }
}

#[async_trait]
impl ToolValidator for BashValidator {
    fn tool_name(&self) -> &'static str {
        BASH_TOOL_NAME
    }

    async fn validate(
        &self,
        tool_call: &ToolCall,
        _context: &ValidationContext,
    ) -> Result<ValidationResult, ValidationError> {
        let params: BashParams = serde_json::from_value(tool_call.parameters.clone())
            .map_err(|e| ValidationError::InvalidParams(e.to_string()))?;

        // First check basic banned commands (fast path)
        if self.is_banned_command(&params.command) {
            return Ok(ValidationResult {
                allowed: false,
                reason: Some(format!(
                    "Command '{}' is disallowed for security reasons",
                    params.command
                )),
                requires_user_approval: false,
            });
        }

        // Command passed all checks
        Ok(ValidationResult {
            allowed: true,
            reason: None,
            requires_user_approval: false,
        })
    }
}
