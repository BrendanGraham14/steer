use anyhow::{Context, Result};
use regex::Regex;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

/// Bash tool implementation
pub struct Bash {
    command_filter: Option<crate::tools::command_filter::CommandFilter>,
}

impl Bash {
    /// Create a new Bash tool
    pub fn new() -> Self {
        Self {
            command_filter: None,
        }
    }

    /// Create a new Bash tool with a command filter
    pub fn with_command_filter(api_key: &str) -> Self {
        Self {
            command_filter: Some(crate::tools::command_filter::CommandFilter::new(api_key)),
        }
    }

    /// Execute a bash command
    pub async fn execute(&self, command: &str) -> Result<String> {
        // Default timeout: 1 hour
        self.execute_with_timeout(command, 3_600_000, None).await
    }

    /// Execute a bash command with cancellation support
    pub async fn execute_with_cancellation(
        &self,
        command: &str,
        token: CancellationToken,
    ) -> Result<String> {
        // Default timeout: 1 hour
        self.execute_with_timeout(command, 3_600_000, Some(token))
            .await
    }

    /// Execute a bash command with a timeout and optional cancellation
    pub async fn execute_with_timeout(
        &self,
        command: &str,
        timeout_ms: u64,
        token: Option<CancellationToken>,
    ) -> Result<String> {
        // First check the basic banned commands
        if is_banned_command(command) {
            return Err(anyhow::anyhow!(
                "This command '{}' is not allowed for security reasons",
                command
            ));
        }
        // If we have a command filter, use it for enhanced security
        if let Some(filter) = &self.command_filter {
            // Check if the command is allowed
            let is_allowed = if let Some(token) = &token {
                filter.is_command_allowed(command, token.clone()).await?
            } else {
                return Err(anyhow::anyhow!(
                    "Command filter is enabled, but no cancellation token was provided to execute_with_timeout"
                ));
            };

            if !is_allowed {
                return Err(anyhow::anyhow!(
                    "This command '{}' was blocked by the command filter. It may contain command injection or use disallowed commands.",
                    command
                ));
            }
        }

        // Check for cancellation before executing
        if let Some(token) = &token {
            if token.is_cancelled() {
                return Err(anyhow::anyhow!(
                    "Command execution was cancelled before starting"
                ));
            }
        }

        // Execute the command with a timeout and cancellation support
        let timeout_duration = Duration::from_millis(timeout_ms);
        let command_owned = command.to_string(); // Clone the command to move into the closure

        // Create the future to execute the command
        let command_future = async {
            let spawn_result = tokio::task::spawn_blocking(move || {
                Command::new("bash")
                    .arg("-c")
                    .arg(command_owned)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
            })
            .await;

            spawn_result
                .context("Failed to execute command")?
                .context("Command execution failed")
        };

        // If we have a cancellation token, use select! to race the command against cancellation
        let result = if let Some(token) = token {
            tokio::select! {
                biased; // Check cancellation first

                _ = token.cancelled() => {
                    return Err(anyhow::anyhow!("Command execution was cancelled"));
                }

                timeout_result = timeout(timeout_duration, command_future) => {
                    timeout_result.context("Command execution timed out")?
                }
            }
        } else {
            // No cancellation token, just execute with timeout
            timeout(timeout_duration, command_future)
                .await
                .context("Command execution timed out")?
        };

        // Handle the Result from command execution
        let result = match result {
            Ok(output) => output,
            Err(e) => return Err(e.context("Command execution failed internally")),
        };

        // Combine stdout and stderr
        let mut output_str = String::from_utf8_lossy(&result.stdout).to_string();

        if !result.stderr.is_empty() {
            if !output_str.is_empty() {
                output_str.push_str("\n\n");
            }
            output_str.push_str("stderr:\n");
            output_str.push_str(&String::from_utf8_lossy(&result.stderr));
        }

        if !result.status.success() {
            output_str.push_str(&format!(
                "\n\nCommand exited with status: {}",
                result.status
            ));
        }

        Ok(output_str)
    }
}

/// Execute a bash command with a timeout
pub async fn execute_bash(command: &str, timeout_ms: u64) -> Result<String> {
    // Create a new Bash tool and execute the command
    let bash = Bash::new();
    bash.execute_with_timeout(command, timeout_ms, None).await
}

/// Execute a bash command with a timeout and cancellation support
pub async fn execute_bash_with_cancellation(
    command: &str,
    timeout_ms: u64,
    token: CancellationToken,
) -> Result<String> {
    // Create a new Bash tool and execute the command with cancellation
    let bash = Bash::new();
    bash.execute_with_timeout(command, timeout_ms, Some(token))
        .await
}

/// Check if a command is banned
fn is_banned_command(command: &str) -> bool {
    let banned_commands = [
        "alias",
        "curl",
        "curlie",
        "wget",
        "axel",
        "aria2c",
        "nc",
        "telnet",
        "lynx",
        "w3m",
        "links",
        "httpie",
        "xh",
        "http-prompt",
        "chrome",
        "firefox",
        "safari",
    ];

    // Check for direct matches at the start of the command
    for banned in banned_commands.iter() {
        // Match only if it's the command, not a substring
        let re = Regex::new(&format!(
            r"^\s*{}\s+|^\s*{}\s*$|^\s*\S*/(bin/)?{}\s+",
            banned, banned, banned
        ))
        .unwrap_or_else(|_| Regex::new(&format!(r"^\s*{}\s", banned)).unwrap());

        if re.is_match(command) {
            return true;
        }
    }

    // Check for command injection patterns
    let command_injection_patterns = [
        r"\s*`.*`",     // Backtick command substitution
        r"\s*\$\(.*\)", // $() command substitution
        r"\s*\|",       // Pipe
        r"\s*&&",       // Command chaining with &&
        r"\s*;",        // Command separator ;
        r"\s*>\s*\S+",  // Redirection >
        r"\s*>>\s*\S+", // Redirection >>
        r"\s*<\s*\S+",  // Redirection <
    ];

    for pattern in command_injection_patterns.iter() {
        if Regex::new(pattern)
            .unwrap_or_else(|_| Regex::new(r"^$").unwrap())
            .is_match(command)
        {
            // Allow specific safe patterns for command chaining
            if pattern == &r"\s*;" || pattern == &r"\s*&&" {
                // Don't ban if it's just chaining basic commands
                let safe_semicolon = Regex::new(r"^[\w\s/._-]+;[\w\s/._-]+$")
                    .unwrap_or_else(|_| Regex::new(r"^$").unwrap());
                let safe_ampersand = Regex::new(r"^[\w\s/._-]+&&[\w\s/._-]+$")
                    .unwrap_or_else(|_| Regex::new(r"^$").unwrap());

                if safe_semicolon.is_match(command) || safe_ampersand.is_match(command) {
                    continue;
                }
            }

            return true;
        }
    }

    false
}
