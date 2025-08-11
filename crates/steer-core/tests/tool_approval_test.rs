type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
use dotenvy::dotenv;
use steer_core::app::{
    App, AppCommand, AppConfig, AppEvent, ApprovalDecision, command::ApprovalType,
};
use steer_core::tools::ToolExecutor;

use serde_json::json;
use std::sync::Arc;
use steer_core::test_utils;
use steer_tools::ToolCall;
use steer_tools::tools::edit::EditTool;
use steer_tools::tools::view::ViewTool;
use steer_tools::tools::{
    BASH_TOOL_NAME, EDIT_TOOL_NAME, GLOB_TOOL_NAME, GREP_TOOL_NAME, LS_TOOL_NAME,
    REPLACE_TOOL_NAME, VIEW_TOOL_NAME,
};
use steer_tools::traits::Tool;
use steer_workspace::local::LocalWorkspace;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};
use tracing::warn; // Added warn import

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
async fn test_requires_approval_tool_detection() -> Result<()> {
    let view_tool = ViewTool;
    let edit_tool = EditTool;
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
    let model_registry = Arc::new(
        steer_core::model_registry::ModelRegistry::load()
            .expect("Failed to load model registry for tests"),
    );
    let app_config = AppConfig {
        llm_config_provider: test_utils::test_llm_config_provider(),
        model_registry,
    };
    let (event_tx, _event_rx) = mpsc::channel::<AppEvent>(100);
    let (workspace, _temp_dir) = create_test_workspace().await;
    let tool_executor = create_test_tool_executor(workspace.clone());

    let app = App::new(
        app_config,
        event_tx,
        steer_core::config::model::builtin::claude_3_7_sonnet_20250219(),
        workspace,
        tool_executor,
        None,
    )
    .await?;

    assert!(
        !app.tool_executor
            .requires_approval(VIEW_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        !app.tool_executor
            .requires_approval(GREP_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        !app.tool_executor
            .requires_approval(LS_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        !app.tool_executor
            .requires_approval(GLOB_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        app.tool_executor
            .requires_approval(EDIT_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        app.tool_executor
            .requires_approval(REPLACE_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        app.tool_executor
            .requires_approval(BASH_TOOL_NAME)
            .await
            .unwrap()
    );
    assert!(
        app.tool_executor
            .requires_approval("non_existent_tool")
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn test_always_approve_cascades_to_pending_tool_calls() -> Result<()> {
    dotenv().ok();
    let app_config_for_actor = test_utils::test_app_config();

    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(100);
    let (command_tx_to_actor, command_rx_for_actor) = mpsc::channel::<AppCommand>(100);

    let (workspace, _temp_dir) = create_test_workspace().await;
    let tool_executor = create_test_tool_executor(workspace.clone());
    let app_for_actor = App::new(
        app_config_for_actor,
        event_tx.clone(),
        steer_core::config::model::builtin::claude_3_7_sonnet_20250219(),
        workspace,
        tool_executor,
        None,
    )
    .await?;
    let actor_handle = tokio::spawn(steer_core::app::app_actor_loop(
        app_for_actor,
        command_rx_for_actor,
    ));

    let tool_name_to_approve = "edit_file".to_string();

    // Tool Call 1
    let tool_call_id_1 = "test_tool_call_id_1".to_string();
    let api_tool_call_1 = ToolCall {
        id: tool_call_id_1.clone(),
        name: tool_name_to_approve.clone(),
        parameters: json!({"path": "/test/file1.txt", "content": "content1"}),
    };
    let (responder_tx_1, responder_rx_1) = oneshot::channel::<ApprovalDecision>();
    command_tx_to_actor
        .send(AppCommand::RequestToolApprovalInternal {
            tool_call: api_tool_call_1.clone(),
            responder: responder_tx_1,
        })
        .await?;

    let event1_option = timeout(Duration::from_secs(2), event_rx.recv()).await?;
    let event1 = event1_option.expect("Event channel closed or no event for call 1");

    // Skip the initial WorkspaceFiles event if present
    let event1 = match event1 {
        AppEvent::WorkspaceFiles { .. } => {
            // Consume the WorkspaceFiles event and wait for the next one
            let next_event_option = timeout(Duration::from_secs(2), event_rx.recv()).await?;
            next_event_option.expect("Event channel closed or no event for call 1")
        }
        other => other,
    };

    match event1 {
        AppEvent::RequestToolApproval { name, id, .. } => {
            assert_eq!(name, tool_name_to_approve);
            assert_eq!(id, tool_call_id_1);
        }
        _ => unreachable!(
            "Unexpected event received instead of RequestToolApproval for call 1: {event1:?}"
        ),
    }

    // Tool Call 2 is sent to the app and queued, but no UI event is expected for it yet,
    // as the first one is still active at the UI.
    let tool_call_id_2 = "test_tool_call_id_2".to_string();
    let api_tool_call_2 = ToolCall {
        id: tool_call_id_2.clone(),
        name: tool_name_to_approve.clone(), // Same tool name
        parameters: json!({"path": "/test/file2.txt", "content": "content2"}),
    };
    let (responder_tx_2, responder_rx_2) = oneshot::channel::<ApprovalDecision>();
    command_tx_to_actor
        .send(AppCommand::RequestToolApprovalInternal {
            tool_call: api_tool_call_2.clone(),
            responder: responder_tx_2,
        })
        .await?;

    // No event is expected for tool_call_id_2 at this point.
    // It will be auto-approved when tool_call_id_1 is 'always' approved.
    // Let's check that no immediate event comes for tool_call_id_2 to be sure.
    match timeout(Duration::from_millis(200), event_rx.recv()).await {
        Ok(Some(unexpected_event)) => {
            match &unexpected_event {
                AppEvent::RequestToolApproval {
                    id: unexpected_id, ..
                } => {
                    if unexpected_id == &tool_call_id_2 {
                        unreachable!(
                            "Received RequestToolApproval for tool_call_id_2 prematurely: {unexpected_event:?}"
                        );
                    }
                }
                AppEvent::WorkspaceFiles { .. } => {
                    // Ignore WorkspaceFiles events
                }
                _ => {
                    // Log if it's some other event, though still unexpected here
                    warn!(
                        "Received an unexpected event while tool_call_id_1 is active: {:?}",
                        unexpected_event
                    );
                }
            }
        }
        Ok(None) => { /* Channel closed, unlikely but possible */ }
        Err(_) => { /* Timeout is expected, good. No event for tool_call_id_2 yet. */ }
    }

    // User "Always Approves" the first tool call (tool_call_id_1)
    command_tx_to_actor
        .send(AppCommand::HandleToolResponse {
            id: tool_call_id_1.clone(),
            approval: ApprovalType::AlwaysTool,
        })
        .await?;

    match timeout(Duration::from_secs(2), responder_rx_1).await? {
        Ok(ApprovalDecision::Approved) => { /* Good */ }
        Ok(ApprovalDecision::Denied) => unreachable!("Tool call 1 was unexpectedly denied."),
        Err(_) => unreachable!("Timeout waiting for decision on tool call 1 responder."),
    }

    match timeout(Duration::from_secs(2), responder_rx_2).await? {
        Ok(ApprovalDecision::Approved) => { /* Good */ }
        Ok(ApprovalDecision::Denied) => {
            unreachable!("Tool call 2 was unexpectedly auto-denied instead of auto-approved.")
        }
        Err(_) => unreachable!(
            "Timeout waiting for decision on tool call 2 responder (should have been auto-approved)."
        ),
    }

    // After 'always' approval of tool_call_id_1, and subsequent auto-approval of tool_call_id_2,
    // there should be no more RequestToolApproval events.
    match timeout(Duration::from_millis(200), event_rx.recv()).await {
        Ok(Some(event)) => match event {
            AppEvent::WorkspaceFiles { .. } | AppEvent::WorkspaceChanged => {
                // Ignore workspace-related events
            }
            AppEvent::RequestToolApproval { .. } => unreachable!(
                "Unexpected third AppEvent::RequestToolApproval received after 'always' approval and cascade: {event:?}"
            ),
            _ => { /* Other events are fine */ }
        },
        Ok(None) => { /* Channel closed, can happen if actor shuts down. */ }
        Err(_) => { /* Timeout is expected, good. */ }
    }

    command_tx_to_actor.send(AppCommand::Shutdown).await?;
    match timeout(Duration::from_secs(1), actor_handle).await {
        Ok(Ok(_)) => { /* Actor shut down cleanly */ }
        Ok(Err(e)) => return Err(format!("Actor task panicked: {e:?}").into()),
        Err(_) => warn!("Timeout waiting for actor to shut down."),
    }

    Ok(())
}

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
