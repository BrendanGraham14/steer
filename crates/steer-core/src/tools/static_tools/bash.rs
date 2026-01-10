use async_trait::async_trait;
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::error::ToolExecutionError;
use steer_tools::result::BashResult;
use steer_tools::tools::BASH_TOOL_NAME;
use steer_tools::tools::bash::BashParams;
use steer_tools::tools::bash::BashError;

pub struct BashTool;

#[async_trait]
impl StaticTool for BashTool {
    type Params = BashParams;
    type Output = BashResult;

    const NAME: &'static str = BASH_TOOL_NAME;
    const DESCRIPTION: &'static str = "Run a bash command in the terminal";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError> {
        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        if is_banned_command(&params.command) {
            return Err(StaticToolError::execution(ToolExecutionError::Bash(
                BashError::DisallowedCommand {
                    command: params.command,
                },
            )));
        }

        let timeout_ms = params.timeout.unwrap_or(3_600_000).min(3_600_000);
        let timeout_duration = Duration::from_millis(timeout_ms);
        let working_directory = ctx.services.workspace.working_directory().to_path_buf();

        tokio::select! {
            _ = ctx.cancellation_token.cancelled() => Err(StaticToolError::Cancelled),
            res = timeout(timeout_duration, run_command(&params.command, &working_directory, ctx.cancellation_token.clone())) => {
                match res {
                    Ok(output) => output,
                    Err(_) => Err(StaticToolError::Timeout),
                }
            }
        }
    }
}

async fn run_command(
    command: &str,
    working_directory: &std::path::Path,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> Result<BashResult, StaticToolError> {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| {
            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                message: e.to_string(),
            }))
        })?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| {
            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                message: "Failed to capture stdout".to_string(),
            }))
        })?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| {
            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                message: "Failed to capture stderr".to_string(),
            }))
        })?;

    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let result = tokio::select! {
        _ = cancellation_token.cancelled() => {
            let _ = child.kill().await;
            stdout_handle.abort();
            stderr_handle.abort();
            Err(StaticToolError::Cancelled)
        }
        status = child.wait() => {
            match status {
                Ok(status) => {
                    let (stdout_result, stderr_result) = tokio::try_join!(stdout_handle, stderr_handle)
                        .map_err(|e| {
                            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                                message: format!("Failed to join read tasks: {e}"),
                            }))
                        })?;

                    let stdout_bytes = stdout_result
                        .map_err(|e| {
                            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                                message: format!("Failed to read stdout: {e}"),
                            }))
                        })?;
                    let stderr_bytes = stderr_result
                        .map_err(|e| {
                            StaticToolError::execution(ToolExecutionError::Bash(BashError::Io {
                                message: format!("Failed to read stderr: {e}"),
                            }))
                        })?;

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
                Err(e) => Err(StaticToolError::execution(ToolExecutionError::Bash(
                    BashError::Io {
                        message: e.to_string(),
                    },
                ))),
            }
        }
    };

    result
}

static BANNED_COMMAND_REGEXES: Lazy<Vec<Regex>> = Lazy::new(|| {
    let banned_commands = [
        "curl", "wget", "nc", "telnet", "ssh", "scp", "ftp", "sftp", "lynx", "w3m", "links",
        "elinks", "httpie", "xh", "chrome", "firefox", "safari", "edge", "opera", "chromium",
        "axel", "aria2c", "alias", "unalias", "exec", "source", ".", "history", "sudo", "su",
        "chown", "chmod", "useradd", "userdel", "groupadd", "groupdel", "vi", "vim", "nano",
        "pico", "emacs", "ed",
    ];

    banned_commands
        .iter()
        .map(|cmd| {
            Regex::new(&format!(r"^\\s*(\\S*/)?{}\\b", regex::escape(cmd)))
                .expect("Failed to compile banned command regex")
        })
        .collect()
});

fn is_banned_command(command: &str) -> bool {
    BANNED_COMMAND_REGEXES.iter().any(|re| re.is_match(command))
}
