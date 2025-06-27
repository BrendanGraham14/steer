use anyhow::Result;
use conductor_core::api::Model;
use conductor_core::app::{App, AppConfig, AppEvent, ToolExecutor};
use conductor_core::config::LlmConfig;
use conductor_core::workspace::local::LocalWorkspace;
use conductor_tools::ToolCall;
use dotenv::dotenv;
use std::sync::Arc;
use tokio::sync::mpsc;

async fn create_test_workspace() -> Arc<LocalWorkspace> {
    Arc::new(
        LocalWorkspace::with_path(std::env::current_dir().unwrap())
            .await
            .unwrap(),
    )
}

fn create_test_tool_executor(workspace: Arc<LocalWorkspace>) -> Arc<ToolExecutor> {
    Arc::new(ToolExecutor::with_workspace(workspace))
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
    let (event_tx, _event_rx) = mpsc::channel::<AppEvent>(100);
    let workspace = create_test_workspace().await;
    let tool_executor = create_test_tool_executor(workspace.clone());
    let app = App::new(
        app_config,
        event_tx,
        Model::Claude3_7Sonnet20250219,
        workspace,
        tool_executor,
        None, // No session config for test
    )?;

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
    let formatted = output.llm_format();
    assert!(!formatted.is_empty(), "Tool output should not be empty");

    println!("Tool result: {}", formatted);
    println!("Tool executor test passed successfully!");
    Ok(())
}
