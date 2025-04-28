use anyhow::Context;
use anyhow::Result;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolError;
use coder_macros::tool;

/// Bash tool implementation
pub struct Bash {
    command_filter: crate::tools::command_filter::CommandFilter,
}

impl Bash {
    /// Create a new Bash tool
    pub fn new() -> Self {
        Self {
            command_filter: crate::tools::command_filter::CommandFilter::new(),
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

        let token = token.unwrap_or_else(CancellationToken::new);

        let is_allowed = self
            .command_filter
            .is_command_allowed(command, token.clone())
            .await?;

        if !is_allowed {
            return Err(anyhow::anyhow!(
                "This command '{}' was blocked by the command filter. It may contain command injection or use disallowed commands.",
                command
            ));
        }

        // Check for cancellation before executing
        if token.is_cancelled() {
            return Err(anyhow::anyhow!(
                "Command execution was cancelled before starting"
            ));
        }

        // Execute the command with a timeout and cancellation support
        let timeout_duration = Duration::from_millis(timeout_ms);
        let command_owned = command.to_string(); // Clone the command to move into the closure

        // Create the future to execute the command
        let command_future = async {
            // Use tokio::process::Command directly and await its output
            Command::new("bash")
                .arg("-c")
                .arg(command_owned) // command_owned is moved here
                .output() // This returns impl Future<Output = std::io::Result<std::process::Output>>
                .await // Await the future directly
                .context("Command execution failed") // Convert io::Result to anyhow::Result
        };

        // If we have a cancellation token, use select! to race the command against cancellation
        let result = {
            tokio::select! {
                biased; // Check cancellation first

                _ = token.cancelled() => {
                    return Err(anyhow::anyhow!("Command execution was cancelled"));
                }

                timeout_result = timeout(timeout_duration, command_future) => {
                    timeout_result.context("Command execution timed out")?
                }
            }
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

// Derive JsonSchema for parameters
#[derive(Deserialize, Debug, JsonSchema)]
struct BashParams {
    /// The command to execute
    command: String,
    /// Optional timeout in milliseconds (default 3600000, max 3600000)
    timeout: Option<u64>,
}

tool! {
    BashTool {
        params: BashParams,
        description: "Run a bash command in the terminal",
        name: "bash"
    }

    async fn run(
        _tool: &BashTool,
        params: BashParams,
        token: Option<CancellationToken>,
    ) -> Result<String, ToolError> {
        // TODO: How to integrate command filtering?
        let timeout_ms = params.timeout.unwrap_or(3_600_000).min(3_600_000);

        if let Some(t) = &token {
            execute_bash_with_cancellation_internal(&params.command, timeout_ms, t.clone()).await
        } else {
            execute_bash_internal(&params.command, timeout_ms).await
        }
    }
}

// --- Internal execution logic (adapted from original mod.rs and bash.rs) ---

async fn execute_bash_internal(command: &str, timeout_ms: u64) -> Result<String, ToolError> {
    let cmd_timeout = Duration::from_millis(timeout_ms);

    match timeout(cmd_timeout, run_command(command)).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(tool_err)) => Err(tool_err),
        Err(_) => Err(ToolError::Timeout("Bash".to_string())),
    }
}

async fn execute_bash_with_cancellation_internal(
    command: &str,
    timeout_ms: u64,
    token: CancellationToken,
) -> Result<String, ToolError> {
    let cmd_timeout = Duration::from_millis(timeout_ms);

    tokio::select! {
        _ = token.cancelled() => {
            Err(ToolError::Cancelled("Bash".to_string()))
        }
        res = timeout(cmd_timeout, run_command(command)) => {
             match res {
                Ok(Ok(output)) => Ok(output),
                Ok(Err(tool_err)) => Err(tool_err),
                Err(_) => Err(ToolError::Timeout("Bash".to_string())),
            }
        }
    }
}

async fn run_command(command: &str) -> Result<String, ToolError> {
    let output_result = Command::new("/bin/bash")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ToolError::Io {
            tool_name: "Bash".to_string(),
            source: e.into(),
        })?
        .wait_with_output()
        .await
        .map_err(|e| ToolError::Io {
            tool_name: "Bash".to_string(),
            source: e.into(),
        })?;

    let stdout = String::from_utf8_lossy(&output_result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output_result.stderr).to_string();

    if output_result.status.success() {
        Ok(stdout)
    } else {
        let exit_code = output_result
            .status
            .code()
            .map_or_else(|| "N/A".to_string(), |c| c.to_string());

        let error_message = format!(
            "Command failed with exit code {}\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
            exit_code,
            stdout.trim(),
            stderr.trim()
        );
        Err(ToolError::Execution {
            tool_name: "Bash".to_string(),
            message: error_message,
        })
    }
}
