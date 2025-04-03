use anyhow::Result;
use coder::app::{App, AppConfig};
use coder::api::ToolCall;
use dotenv::dotenv;
use std::env;

#[tokio::test]
#[ignore]
async fn test_app_initialization() -> Result<()> {
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

    // Create app config
    let app_config = AppConfig {
        api_key,
    };

    // Initialize the app
    let app = App::new(app_config)?;
    
    // Verify the app was initialized correctly
    assert!(!app.env_info.working_directory.as_os_str().is_empty(), "Working directory should not be empty");
    
    println!("App initialization test passed successfully!");
    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_tool_executor() -> Result<()> {
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

    // Create app config
    let app_config = AppConfig {
        api_key,
    };

    // Initialize the app
    let app = App::new(app_config)?;
    
    // Create a tool call for listing the current directory
    let parameters = serde_json::json!({
        "path": "."
    });
    
    let tool_call = ToolCall {
        name: "LS".to_string(),
        parameters,
        id: Some("test-ls-call".to_string()),
    };
    
    // Execute the tool
    let result = app.execute_tool(&tool_call).await;
    
    // Verify the tool executed correctly
    assert!(result.is_ok(), "Tool execution failed: {:?}", result.err());
    let output = result?;
    assert!(!output.is_empty(), "Tool output should not be empty");
    
    println!("Tool result: {}", output);
    println!("Tool executor test passed successfully!");
    Ok(())
}