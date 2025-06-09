use anyhow::Result;
use coder::api::{Model, ToolCall};
use coder::app::{App, AppConfig, ToolExecutor};
use coder::app::validation::ValidatorRegistry;
use coder::config::LlmConfig;
use coder::tools::{BackendRegistry, LocalBackend};
use dotenv::dotenv;
use std::sync::Arc;
use tokio::sync::mpsc;

fn create_test_tool_executor() -> Arc<ToolExecutor> {
    let mut backend_registry = BackendRegistry::new();
    backend_registry.register(
        "local".to_string(),
        Arc::new(LocalBackend::full()),
    );
    
    Arc::new(ToolExecutor {
        backend_registry: Arc::new(backend_registry),
        validators: Arc::new(ValidatorRegistry::new()),
    })
}

#[tokio::test]
#[ignore]
async fn test_tool_executor() -> Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Create app config
    let config = LlmConfig::from_env().unwrap();
    let app_config = AppConfig { llm_config: config };

    // Initialize the app
    // Create a channel for app events
    let (event_tx, _event_rx) = mpsc::channel(100);
    let app = App::new(app_config, event_tx, Model::Claude3_7Sonnet20250219, create_test_tool_executor())?;

    // Create a tool call for listing the current directory
    let parameters = serde_json::json!({
        "path": "."
    });

    let tool_call = ToolCall {
        name: "LS".to_string(),
        parameters,
        id: "test-ls-call".to_string(),
    };

    // Execute the tool with cancellation token
    let result = app
        .tool_executor
        .execute_tool_with_cancellation(&tool_call, tokio_util::sync::CancellationToken::new())
        .await;

    // Verify the tool executed correctly
    assert!(result.is_ok(), "Tool execution failed: {:?}", result.err());
    let output = result?;
    assert!(!output.is_empty(), "Tool output should not be empty");

    println!("Tool result: {}", output);
    println!("Tool executor test passed successfully!");
    Ok(())
}
