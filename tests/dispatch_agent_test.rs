use anyhow::Result;
use coder::tools::dispatch_agent::DispatchAgent;
use dotenv::dotenv;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore]
async fn test_dispatch_agent() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Create the dispatch agent with the API key
    let agent = DispatchAgent::new()?;

    // Test prompt that should search for specific code
    let prompt = "Find all files that contain definitions of functions or methods related to search or find operations";

    // Execute the agent
    let result = agent.execute(prompt, CancellationToken::new()).await;

    // Check if we got a valid response
    assert!(result.is_ok(), "Agent execution failed: {:?}", result.err());
    let response = result?;
    assert!(!response.is_empty(), "Response should not be empty");

    println!("Dispatch agent response: {}", response);
    println!("Dispatch agent test passed successfully!");

    Ok(())
}
