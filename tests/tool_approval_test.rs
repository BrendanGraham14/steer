use anyhow::Result;
use coder::api::Model;
use coder::app::{App, AppConfig};
use coder::config::LlmConfig;
use coder::tools::edit::EditTool;
use coder::tools::traits::Tool;
use coder::tools::view::ViewTool;
use dotenv::dotenv;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_requires_approval_tool_detection() -> Result<()> {
    // Create read-only and write tools
    let view_tool = ViewTool;
    let edit_tool = EditTool;

    // Test is_read_only implementation
    assert!(
        !view_tool.requires_approval(),
        "ViewTool should not require approval"
    );
    assert!(
        edit_tool.requires_approval(),
        "EditTool should require approval"
    );

    Ok(())
}

#[tokio::test]
async fn test_tool_executor_requires_approval_check() -> Result<()> {
    dotenv().ok();

    let llm_config = LlmConfig::from_env()?;
    let app_config = AppConfig { llm_config };
    let (event_tx, _event_rx) = mpsc::channel(100);
    let app = App::new(app_config, event_tx, Model::Claude3_7Sonnet20250219)?;

    // Check read-only status through the tool executor
    assert!(
        !app.tool_executor.requires_approval("read_file").unwrap(),
        "read_file should not require approval by default"
    );
    assert!(
        !app.tool_executor.requires_approval("grep").unwrap(),
        "grep should not require approval by default"
    );
    assert!(
        !app.tool_executor.requires_approval("ls").unwrap(),
        "ls should not require approval by default"
    );
    assert!(
        !app.tool_executor.requires_approval("glob").unwrap(),
        "glob should not require approval by default"
    );
    assert!(
        app.tool_executor.requires_approval("web_fetch").unwrap(),
        "web_fetch should require approval by default"
    );

    assert!(
        app.tool_executor.requires_approval("edit_file").unwrap(),
        "edit_file should require approval by default"
    );
    assert!(
        app.tool_executor.requires_approval("write_file").unwrap(),
        "replace_file should require approval by default"
    );
    assert!(
        app.tool_executor.requires_approval("bash").unwrap(),
        "bash should require approval by default"
    );

    // Test non-existent tool
    assert!(
        app.tool_executor
            .requires_approval("non_existent_tool")
            .is_err(),
        "Non-existent tool should throw an error"
    );

    Ok(())
}

// Note: The following test would require more complex setup with mocking
// to test the actual initiate_tool_calls flow and the always-approve behavior
// This is left as a comment for future implementation
/*
#[tokio::test]
async fn test_tool_approval_flow() -> Result<()> {
    // This would test:
    // 1. Read-only tools auto-approved
    // 2. Always-approve flag persisting approvals
    // Would need mocking of the actual tool execution and app state
    Ok(())
}
*/
