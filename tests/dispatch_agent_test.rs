use anyhow::Result;
use coder::tools::dispatch_agent::DispatchAgent;
use dotenv::dotenv;
use std::env;

#[tokio::test]
#[ignore]
async fn test_dispatch_agent() -> Result<()> {
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

    // Create the dispatch agent with the API key
    let agent = DispatchAgent::with_api_key(api_key);
    
    // Test prompt that should search for specific code
    let prompt = "Find all files that contain definitions of functions or methods related to search or find operations";
    
    // Execute the agent
    let result = agent.execute(prompt).await;
    
    // Check if we got a valid response
    assert!(result.is_ok(), "Agent execution failed: {:?}", result.err());
    let response = result?;
    assert!(!response.is_empty(), "Response should not be empty");
    
    println!("Dispatch agent response: {}", response);
    println!("Dispatch agent test passed successfully!");
    
    Ok(())
}