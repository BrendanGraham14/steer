use async_trait::async_trait;
use regex::Regex;
use std::process::ExitStatus;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::tools::capability::Capabilities;
use crate::tools::static_tool::{StaticTool, StaticToolContext, StaticToolError};
use steer_tools::result::BashResult;
use steer_tools::tools::bash::{BashError, BashParams, BashToolSpec};

const DEFAULT_TIMEOUT_MS: u64 = 180_000;
const MAX_TIMEOUT_MS: u64 = 3_600_000;
const TIMEOUT_EXIT_CODE: i32 = 124;

pub struct BashTool;

#[async_trait]
impl StaticTool for BashTool {
    type Params = BashParams;
    type Output = BashResult;
    type Spec = BashToolSpec;

    const DESCRIPTION: &'static str = "Run a bash command in the terminal";
    const REQUIRES_APPROVAL: bool = true;
    const REQUIRED_CAPABILITIES: Capabilities = Capabilities::WORKSPACE;

    async fn execute(
        &self,
        params: Self::Params,
        ctx: &StaticToolContext,
    ) -> Result<Self::Output, StaticToolError<BashError>> {
        if ctx.is_cancelled() {
            return Err(StaticToolError::Cancelled);
        }

        if is_banned_command(&params.command) {
            return Err(StaticToolError::execution(BashError::DisallowedCommand {
                command: params.command,
            }));
        }

        let timeout_ms = params
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let timeout_duration = Duration::from_millis(timeout_ms);
        let working_directory = ctx.services.workspace.working_directory().to_path_buf();

        run_command(
            &params.command,
            &working_directory,
            timeout_duration,
            ctx.cancellation_token.clone(),
        )
        .await
    }
}

enum CommandCompletion {
    Completed(ExitStatus),
    TimedOut,
}

async fn run_command(
    command: &str,
    working_directory: &std::path::Path,
    timeout_duration: Duration,
    cancellation_token: tokio_util::sync::CancellationToken,
) -> Result<BashResult, StaticToolError<BashError>> {
    let mut cmd = Command::new("/bin/bash");
    cmd.arg("-c")
        .arg(command)
        .current_dir(working_directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        StaticToolError::execution(BashError::Io {
            message: e.to_string(),
        })
    })?;

    let mut stdout = child.stdout.take().ok_or_else(|| {
        StaticToolError::execution(BashError::Io {
            message: "Failed to capture stdout".to_string(),
        })
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        StaticToolError::execution(BashError::Io {
            message: "Failed to capture stderr".to_string(),
        })
    })?;

    let stdout_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout.read_to_end(&mut buf).await.map(|_| buf)
    });

    let stderr_handle = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr.read_to_end(&mut buf).await.map(|_| buf)
    });

    let completion = tokio::select! {
        () = cancellation_token.cancelled() => {
            let _ = child.kill().await;
            stdout_handle.abort();
            stderr_handle.abort();
            return Err(StaticToolError::Cancelled);
        }
        status = child.wait() => {
            let status = status.map_err(|e| {
                StaticToolError::execution(BashError::Io {
                    message: e.to_string(),
                })
            })?;
            CommandCompletion::Completed(status)
        }
        () = tokio::time::sleep(timeout_duration) => {
            let _ = child.kill().await;
            child.wait().await.map_err(|e| {
                StaticToolError::execution(BashError::Io {
                    message: format!("Failed to wait for timed out command: {e}"),
                })
            })?;
            CommandCompletion::TimedOut
        }
    };

    let (stdout_result, stderr_result) =
        tokio::try_join!(stdout_handle, stderr_handle).map_err(|e| {
            StaticToolError::execution(BashError::Io {
                message: format!("Failed to join read tasks: {e}"),
            })
        })?;

    let stdout_bytes = stdout_result.map_err(|e| {
        StaticToolError::execution(BashError::Io {
            message: format!("Failed to read stdout: {e}"),
        })
    })?;
    let stderr_bytes = stderr_result.map_err(|e| {
        StaticToolError::execution(BashError::Io {
            message: format!("Failed to read stderr: {e}"),
        })
    })?;

    let stdout = String::from_utf8_lossy(&stdout_bytes).to_string();
    let stderr = String::from_utf8_lossy(&stderr_bytes).to_string();

    let (exit_code, timed_out) = match completion {
        CommandCompletion::Completed(status) => (status.code().unwrap_or(-1), false),
        CommandCompletion::TimedOut => (TIMEOUT_EXIT_CODE, true),
    };

    Ok(BashResult {
        stdout,
        stderr,
        exit_code,
        command: command.to_string(),
        timed_out,
    })
}

static BANNED_COMMAND_REGEXES: std::sync::LazyLock<Vec<Regex>> = std::sync::LazyLock::new(|| {
    let banned_commands = [
        "curl", "wget", "nc", "telnet", "ssh", "scp", "ftp", "sftp", "lynx", "w3m", "links",
        "elinks", "httpie", "xh", "chrome", "firefox", "safari", "edge", "opera", "chromium",
        "axel", "aria2c", "alias", "unalias", "exec", "source", ".", "history", "sudo", "su",
        "chown", "chmod", "useradd", "userdel", "groupadd", "groupdel", "vi", "vim", "nano",
        "pico", "emacs", "ed",
    ];

    banned_commands
        .iter()
        .filter_map(|cmd| {
            let pattern = format!(r"^\\s*(\\S*/)?{}\\b", regex::escape(cmd));
            match Regex::new(&pattern) {
                Ok(regex) => Some(regex),
                Err(err) => {
                    tracing::error!(
                        target: "tools::bash",
                        command = %cmd,
                        error = %err,
                        "Failed to compile banned command regex"
                    );
                    None
                }
            }
        })
        .collect()
});

fn is_banned_command(command: &str) -> bool {
    BANNED_COMMAND_REGEXES.iter().any(|re| re.is_match(command))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{TIMEOUT_EXIT_CODE, run_command};

    #[tokio::test]
    async fn returns_partial_output_when_command_times_out() {
        let result = run_command(
            "echo hello; sleep 1; echo world",
            Path::new("."),
            Duration::from_millis(100),
            CancellationToken::new(),
        )
        .await
        .expect("timed out command should still return output");

        assert!(result.timed_out);
        assert_eq!(result.exit_code, TIMEOUT_EXIT_CODE);
        assert!(result.stdout.contains("hello"));
        assert!(!result.stdout.contains("world"));
    }

    #[tokio::test]
    async fn marks_successful_command_as_not_timed_out() {
        let result = run_command(
            "printf 'done'",
            Path::new("."),
            Duration::from_secs(5),
            CancellationToken::new(),
        )
        .await
        .expect("command should complete");

        assert!(!result.timed_out);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "done");
    }
}
