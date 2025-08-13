type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
use dotenvy::dotenv;
use steer_core::app::{App, AppConfig, AppEvent};
use steer_core::tools::ToolExecutor;

use std::sync::Arc;
use steer_core::test_utils;
use steer_tools::ToolCall;
use steer_workspace::local::LocalWorkspace;
use tempfile::TempDir;
use tokio::sync::mpsc;

async fn create_test_workspace() -> (Arc<LocalWorkspace>, TempDir) {
    let temp_dir = TempDir::new().unwrap();
    let workspace = Arc::new(
        LocalWorkspace::with_path(temp_dir.path().to_path_buf())
            .await
            .unwrap(),
    );
    (workspace, temp_dir)
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
    let model_registry = Arc::new(
        steer_core::model_registry::ModelRegistry::load(&[])
            .expect("Failed to load model registry for tests"),
    );
    let provider_registry = Arc::new(
        steer_core::auth::ProviderRegistry::load(&[])
            .expect("Failed to load provider registry for tests"),
    );
    let app_config = AppConfig {
        llm_config_provider: test_utils::test_llm_config_provider(),
        model_registry,
        provider_registry,
    };

    // Initialize the app
    // Create a channel for app events
    let (event_tx, _event_rx) = mpsc::channel::<AppEvent>(100);
    let (workspace, _temp_dir) = create_test_workspace().await;
    let tool_executor = create_test_tool_executor(workspace.clone());
    let app = App::new(
        app_config,
        event_tx,
        steer_core::config::model::builtin::claude_3_7_sonnet_20250219(),
        workspace,
        tool_executor,
        None, // No session config for test
    )
    .await?;

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

    println!("Tool result: {formatted}");
    println!("Tool executor test passed successfully!");
    Ok(())
}
