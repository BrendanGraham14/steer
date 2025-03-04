use anyhow::{Context, Result};
use std::process::Command;
use tokio::time::timeout;
use std::time::Duration;
use regex::Regex;

/// Execute a bash command with a timeout
pub async fn execute_bash(command: &str, timeout_ms: u64) -> Result<String> {
    // Check for banned commands
    if is_banned_command(command) {
        return Err(anyhow::anyhow!("This command is not allowed for security reasons"));
    }

    // Execute the command with a timeout
    let timeout_duration = Duration::from_millis(timeout_ms);
    let command_owned = command.to_string(); // Clone the command to move into the closure
    let result = timeout(timeout_duration, tokio::task::spawn_blocking(move || {
        Command::new("bash")
            .arg("-c")
            .arg(command_owned)
            .output()
    })).await
      .context("Command execution timed out")?
      .context("Failed to execute command")?
      .context("Command execution failed")?;

    // Combine stdout and stderr
    let mut output = String::from_utf8_lossy(&result.stdout).to_string();
    
    if !result.stderr.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("stderr:\n");
        output.push_str(&String::from_utf8_lossy(&result.stderr));
    }

    if !result.status.success() {
        output.push_str(&format!("\n\nCommand exited with status: {}", result.status));
    }

    Ok(output)
}

/// Check if a command is banned
fn is_banned_command(command: &str) -> bool {
    let banned_commands = [
        "alias", "curl", "curlie", "wget", "axel", "aria2c", 
        "nc", "telnet", "lynx", "w3m", "links", "httpie", 
        "xh", "http-prompt", "chrome", "firefox", "safari"
    ];

    // Check for direct matches at the start of the command
    for banned in banned_commands.iter() {
        // Match only if it's the command, not a substring
        let re = Regex::new(&format!(r"^\s*{}\s+|^\s*{}\s*$|^\s*\S*/(bin/)?{}\s+", 
                                     banned, banned, banned))
            .unwrap_or_else(|_| Regex::new(&format!(r"^\s*{}\s", banned)).unwrap());
        
        if re.is_match(command) {
            return true;
        }
    }

    // Check for command injection patterns
    let command_injection_patterns = [
        r"\s*`.*`",          // Backtick command substitution
        r"\s*\$\(.*\)",      // $() command substitution
        r"\s*\|",            // Pipe
        r"\s*&&",            // Command chaining with &&
        r"\s*;",             // Command separator ;
        r"\s*>\s*\S+",       // Redirection >
        r"\s*>>\s*\S+",      // Redirection >>
        r"\s*<\s*\S+",       // Redirection <
    ];

    for pattern in command_injection_patterns.iter() {
        if Regex::new(pattern).unwrap_or_else(|_| Regex::new(r"^$").unwrap()).is_match(command) {
            // Allow specific safe patterns for command chaining
            if pattern == &r"\s*;" || pattern == &r"\s*&&" {
                // Don't ban if it's just chaining basic commands
                let safe_semicolon = Regex::new(r"^[\w\s/._-]+;[\w\s/._-]+$").unwrap_or_else(|_| Regex::new(r"^$").unwrap());
                let safe_ampersand = Regex::new(r"^[\w\s/._-]+&&[\w\s/._-]+$").unwrap_or_else(|_| Regex::new(r"^$").unwrap());
                
                if safe_semicolon.is_match(command) || safe_ampersand.is_match(command) {
                    continue;
                }
            }
            
            return true;
        }
    }

    false
}