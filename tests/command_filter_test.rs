use anyhow::Result;
use coder::tools::command_filter::CommandFilter;
use dotenv::dotenv;
use std::env;

#[tokio::test]
#[ignore]
async fn test_command_filter() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Get API key from environment
    let api_key = match env::var("CLAUDE_API_KEY") {
        Ok(key) => key,
        Err(_) => {
            println!("CLAUDE_API_KEY not found in environment. Skipping test.");
            return Ok(());
        }
    };

    // Create the command filter
    let filter = CommandFilter::new(&api_key);
    
    // Test known safe commands
    let safe_commands = [
        "ls -la",
        "cd /tmp",
        "pwd",
        "echo 'hello world'",
        "mkdir test_dir",
        "git status",
        "git diff HEAD~1",
        "cargo build",
        "cargo test",
    ];
    
    for cmd in &safe_commands {
        let is_allowed = filter.is_command_allowed(cmd).await?;
        assert!(is_allowed, "Command should be allowed: {}", cmd);
        println!("Safe command allowed: {}", cmd);
    }
    
    // Test known unsafe commands
    let unsafe_commands = [
        "curl https://example.com",
        "wget example.com",
        "rm -rf /",
        "ls -la | grep secret",
        "echo 'hello' > /etc/passwd",
        "git status; cat /etc/shadow",
        "ls `cat /etc/passwd`",
        "ls $(pwd)/etc",
    ];
    
    for cmd in &unsafe_commands {
        let is_allowed = filter.is_command_allowed(cmd).await?;
        assert!(!is_allowed, "Command should be blocked: {}", cmd);
        println!("Unsafe command blocked: {}", cmd);
    }
    
    println!("Command filter test passed successfully!");
    Ok(())
}