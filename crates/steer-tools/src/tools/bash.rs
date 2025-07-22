use once_cell::sync::Lazy;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use steer_macros::tool;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::result::BashResult;
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
        output: BashResult,
        variant: Bash,
        description: "Run a bash command in the terminal",
        name: "bash",
        require_approval: true
    }

    async fn run(
        _tool: &BashTool,
        params: BashParams,
        context: &ExecutionContext,
    ) -> Result<BashResult, ToolError> {
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

async fn run_command(command: &str, context: &ExecutionContext) -> Result<BashResult, ToolError> {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(&context.working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true); // This ensures the child is killed when dropped

    // Set environment variables
    // TODO: Do we want to do this?
    for (key, value) in &context.environment {
        cmd.env(key, value);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::io(BASH_TOOL_NAME, e.to_string()))?;

    // Take the stdout and stderr handles
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::io(BASH_TOOL_NAME, "Failed to capture stdout".to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::io(BASH_TOOL_NAME, "Failed to capture stderr".to_string()))?;

    // Read stdout and stderr concurrently with the process execution
    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    // Wait for the process to complete, with cancellation support
    let result = tokio::select! {
        _ = context.cancellation_token.cancelled() => {
            // The child will be killed automatically when dropped due to kill_on_drop(true)
            // But we can also explicitly kill it to be sure
            let _ = child.kill().await;
            // Also abort the read tasks
            stdout_handle.abort();
            stderr_handle.abort();
            return Err(ToolError::Cancelled(BASH_TOOL_NAME.to_string()));
        }
        status = child.wait() => {
            match status {
                Ok(status) => {
                    // Now collect the output that was already being read concurrently
                    let (stdout_result, stderr_result) = tokio::try_join!(stdout_handle, stderr_handle)
                        .map_err(|e| ToolError::io(BASH_TOOL_NAME, format!("Failed to join read tasks: {e}")))?;

                    let stdout_bytes = stdout_result.map_err(|e|
                        ToolError::io(BASH_TOOL_NAME, format!("Failed to read stdout: {e}"))
                    )?;
                    let stderr_bytes = stderr_result.map_err(|e|
                        ToolError::io(BASH_TOOL_NAME, format!("Failed to read stderr: {e}"))
                    )?;

                    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
                    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();
                    let exit_code = status.code().unwrap_or(-1);

                    Ok(BashResult {
                        stdout,
                        stderr,
                        exit_code,
                        command: command.to_string(),
                    })
                }
                Err(e) => Err(ToolError::io(BASH_TOOL_NAME, e.to_string()))
            }
        }
    };

    result
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
