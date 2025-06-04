use macros::tool;
use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

use crate::{ExecutionContext, ToolError};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BashParams {
    /// The command to execute
    pub command: String,
    /// Optional timeout in milliseconds (default 3600000, max 3600000)
    #[schemars(range(min = 1, max = 3600000))]
    pub timeout: Option<u64>,
}

tool! {
    BashTool {
        params: BashParams,
        description: "Run a bash command in the terminal",
        name: "bash",
        require_approval: true
    }

    async fn run(
        _tool: &BashTool,
        params: BashParams,
        context: &ExecutionContext,
    ) -> Result<String, ToolError> {
        if context.is_cancelled() {
            return Err(ToolError::Cancelled(BASH_TOOL_NAME.to_string()));
        }

        // Basic security check
        if is_banned_command(&params.command) {
            return Err(ToolError::execution(
                BASH_TOOL_NAME,
                format!(
                    "Command '{}' is disallowed for security reasons",
                    params.command
                ),
            ));
        }

        let timeout_ms = params.timeout.unwrap_or(3_600_000).min(3_600_000);
        let timeout_duration = Duration::from_millis(timeout_ms);

        // Execute the command with cancellation support
        let result = tokio::select! {
            _ = context.cancellation_token.cancelled() => {
                return Err(ToolError::Cancelled(BASH_TOOL_NAME.to_string()));
            }
            res = timeout(timeout_duration, run_command(&params.command, context)) => {
                match res {
                    Ok(output) => output,
                    Err(_) => return Err(ToolError::Timeout(BASH_TOOL_NAME.to_string())),
                }
            }
        };

        result
    }
}

async fn run_command(command: &str, context: &ExecutionContext) -> Result<String, ToolError> {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&context.working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Set environment variables
    // TODO: Do we want to do this?
    for (key, value) in &context.environment {
        cmd.env(key, value);
    }

    let output = cmd
        .spawn()
        .map_err(|e| ToolError::io(BASH_TOOL_NAME, e.to_string()))?
        .wait_with_output()
        .await
        .map_err(|e| ToolError::io(BASH_TOOL_NAME, e.to_string()))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(stdout)
    } else {
        let exit_code = output
            .status
            .code()
            .map_or_else(|| "N/A".to_string(), |c| c.to_string());

        let error_message = format!(
            "Command failed with exit code {}\n--- STDOUT ---\n{}\n--- STDERR ---\n{}",
            exit_code,
            stdout.trim(),
            stderr.trim()
        );
        Err(ToolError::execution(BASH_TOOL_NAME, error_message))
    }
}

static BANNED_COMMAND_REGEXES: Lazy<Vec<Regex>> = Lazy::new(|| {
    let banned_commands = [
        // Network tools
        "curl", "wget", "nc", "telnet", "ssh", "scp", "ftp", "sftp",
        // Web browsers/clients
        "lynx", "w3m", "links", "elinks", "httpie", "xh", "chrome", "firefox", "safari", "edge",
        "opera", "chromium", // Download managers
        "axel", "aria2c", // Shell utilities that might be risky
        "alias", "unalias", "exec", "source", ".", "history",
        // Potentially dangerous system modification tools
        "sudo", "su", "chown", "chmod", "useradd", "userdel", "groupadd", "groupdel",
        // File editors
        "vi", "vim", "nano", "pico", "emacs", "ed",
    ];

    banned_commands
        .iter()
        .map(|cmd| {
            Regex::new(&format!(r"^\s*(\S*/)?{}\b", regex::escape(cmd)))
                .expect("Failed to compile banned command regex")
        })
        .collect()
});

fn is_banned_command(command: &str) -> bool {
    BANNED_COMMAND_REGEXES.iter().any(|re| re.is_match(command))
}
