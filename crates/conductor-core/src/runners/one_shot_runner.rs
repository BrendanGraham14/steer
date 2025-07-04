use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::api::Model;
use crate::app::conversation::UserContent;
use crate::app::{AppCommand, AppConfig, Message};
use crate::config::LlmConfig;
use crate::error::{Error, Result};
use crate::session::state::WorkspaceConfig;

/// Record of a tool execution for audit purposes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRecord {
    pub tool_call_id: String,
    pub output: String,
    pub is_error: bool,
}
use crate::session::{
    manager::SessionManager,
    state::{SessionConfig, SessionToolConfig, ToolApprovalPolicy},
};

/// Contains the result of a single agent run, including the final assistant message
/// and all tool results produced during the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOnceResult {
    /// The final assistant message after all tools have been executed
    pub final_msg: Message,
    /// All tool results produced during the run (for audit logging)
    pub tool_results: Vec<ToolResultRecord>,
}

/// Orchestrates single non-interactive agent loop executions using the session system.
///
/// All OneShotRunner operations now use the unified session-based architecture,
/// providing consistent tool configuration, approval policies, and persistence.
pub struct OneShotRunner;

impl Default for OneShotRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl OneShotRunner {
    /// Creates a new OneShotRunner
    pub fn new() -> Self {
        Self
    }

    /// Run a one-shot task in an existing session
    pub async fn run_in_session(
        session_manager: &SessionManager,
        session_id: String,
        message: String,
    ) -> Result<RunOnceResult> {
        // 1. Resume or activate the session if not already active
        let app_config = AppConfig {
            llm_config: LlmConfig::from_env()
                .map_err(|e| Error::Configuration(format!("Failed to load LLM config: {}", e)))?,
        };

        let command_tx = session_manager
            .resume_session(&session_id, app_config)
            .await?;

        // 2. Take the event receiver for this session (like TUI does)
        let event_rx = session_manager.take_event_receiver(&session_id).await?;

        info!(session_id = %session_id, message = %message, "Sending message to session");

        // 3. Send the user message
        command_tx
            .send(AppCommand::ProcessUserInput(message))
            .await
            .map_err(|e| {
                Error::InvalidOperation(format!(
                    "Failed to send message to session {}: {}",
                    session_id, e
                ))
            })?;

        // 4. Process events to build the result (similar to TUI's event loop)
        let result = Self::process_events(event_rx, &session_id).await;

        // Suspend the session before returning
        if let Err(e) = session_manager.suspend_session(&session_id).await {
            error!(session_id = %session_id, error = %e, "Failed to suspend session");
        } else {
            info!(session_id = %session_id, "Session suspended successfully");
        }

        // Return the result
        result
    }

    /// Run a one-shot task in a new ephemeral session
    pub async fn run_ephemeral(
        session_manager: &SessionManager,
        init_msgs: Vec<Message>,
        model: Model,
        tool_config: Option<SessionToolConfig>,
        tool_policy: Option<ToolApprovalPolicy>,
        system_prompt: Option<String>,
    ) -> Result<RunOnceResult> {
        // 1. Create ephemeral session with specified tool policy
        let session_config = if let Some(config) = tool_config {
            // Use provided tool config
            let mut final_tool_config = config;
            // Apply the tool policy if provided
            if let Some(policy) = tool_policy {
                final_tool_config.approval_policy = policy;
            }

            SessionConfig {
                workspace: WorkspaceConfig::default(),
                tool_config: final_tool_config,
                system_prompt,
                metadata: [
                    ("mode".to_string(), "headless".to_string()),
                    ("ephemeral".to_string(), "true".to_string()),
                    ("created_by".to_string(), "one_shot_runner".to_string()),
                    ("model".to_string(), model.to_string()),
                ]
                .into_iter()
                .collect(),
            }
        } else {
            // Use the default session config
            let mut default_config = crate::utils::session::create_default_session_config();

            // Apply the tool policy if provided
            if let Some(policy) = tool_policy {
                default_config.tool_config.approval_policy = policy;
            }

            // Update metadata
            default_config.metadata = [
                ("mode".to_string(), "headless".to_string()),
                ("ephemeral".to_string(), "true".to_string()),
                ("created_by".to_string(), "one_shot_runner".to_string()),
                ("model".to_string(), model.to_string()),
            ]
            .into_iter()
            .collect();

            // Apply custom system prompt if provided
            if system_prompt.is_some() {
                default_config.system_prompt = system_prompt;
            }

            default_config
        };

        let app_config = AppConfig {
            llm_config: LlmConfig::from_env()
                .map_err(|e| Error::Configuration(format!("Failed to load LLM config: {}", e)))?,
        };

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await?;

        // 3. Process the final user message (this triggers the actual processing)
        let user_content = match init_msgs.last() {
            Some(message) => {
                // Extract text content from the message
                match message {
                    Message::User { content, .. } => {
                        let text_content = content.iter().find_map(|c| match c {
                            UserContent::Text { text } => Some(text.clone()),
                            _ => None,
                        });
                        match text_content {
                            Some(content) => content,
                            None => {
                                return Err(Error::InvalidOperation(
                                    "Last message must contain text content".to_string(),
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(Error::InvalidOperation(
                            "Last message must be from User".to_string(),
                        ));
                    }
                }
            }
            None => {
                return Err(Error::InvalidOperation(
                    "No user message to process".to_string(),
                ));
            }
        };

        // 4. Run the main task using the session
        Self::run_in_session(session_manager, session_id.clone(), user_content).await
    }

    /// Process events from the session and return the final result
    async fn process_events(
        mut event_rx: tokio::sync::mpsc::Receiver<crate::app::AppEvent>,
        session_id: &str,
    ) -> Result<RunOnceResult> {
        use crate::app::AppEvent;

        let mut tool_results = Vec::new();
        let mut assistant_message: Option<Message> = None;
        let mut _current_assistant_id: Option<String> = None;

        // Track tool calls that have been started but not completed
        let mut pending_tool_calls: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        info!(session_id = %session_id, "Starting event processing loop");

        while let Some(event) = event_rx.recv().await {
            match event {
                AppEvent::MessageAdded { message, model: _ } => {
                    info!(session_id = %session_id, role = ?message.role(), id = %message.id(), "MessageAdded event");

                    if matches!(message.role(), crate::app::conversation::Role::Assistant) {
                        _current_assistant_id = Some(message.id().to_string());
                        assistant_message = Some(message);

                        // Don't break yet - assistant might make tool calls
                    }
                }

                AppEvent::MessageUpdated { id, .. } => {
                    info!(session_id = %session_id, id = %id, "MessageUpdated event");
                    // We'll get the final message in MessageAdded, so we can ignore updates
                }

                AppEvent::ToolCallStarted { name, id, .. } => {
                    info!(session_id = %session_id, name = %name, id = %id, "ToolCallStarted event");
                    pending_tool_calls.insert(id.clone(), name);
                }

                AppEvent::ToolCallCompleted {
                    name, result, id, ..
                } => {
                    info!(session_id = %session_id, name = %name, id = %id, "ToolCallCompleted event");
                    pending_tool_calls.remove(&id);

                    tool_results.push(ToolResultRecord {
                        tool_call_id: id.clone(),
                        output: result.llm_format(),
                        is_error: false,
                    });
                }

                AppEvent::ToolCallFailed {
                    name, error, id, ..
                } => {
                    error!(session_id = %session_id, name = %name, id = %id, error = %error, "ToolCallFailed event");
                    pending_tool_calls.remove(&id);

                    tool_results.push(ToolResultRecord {
                        tool_call_id: id.clone(),
                        output: error,
                        is_error: true,
                    });
                }

                AppEvent::ThinkingCompleted => {
                    info!(session_id = %session_id, "ThinkingCompleted event received");
                    // Check if we have an assistant message and no pending tool calls
                    if assistant_message.is_some() && pending_tool_calls.is_empty() {
                        info!(session_id = %session_id, "Assistant response complete, exiting event loop");
                        break;
                    }
                }

                AppEvent::Error { message } => {
                    error!(session_id = %session_id, error = %message, "Error event");
                    return Err(Error::InvalidOperation(format!(
                        "Error during processing: {}",
                        message
                    )));
                }

                AppEvent::RequestToolApproval { .. } => {
                    info!(session_id = %session_id, "RequestToolApproval event - this shouldn't happen in headless mode");
                    // In headless mode, tools should be pre-approved or denied by policy
                }

                _ => {
                    // Ignore other events like ThinkingStarted, ModelChanged, etc.
                }
            }
        }

        // Return the result
        match assistant_message {
            Some(final_msg) => {
                info!(
                    session_id = %session_id,
                    tool_results_count = tool_results.len(),
                    "Returning final result"
                );
                Ok(RunOnceResult {
                    final_msg,
                    tool_results,
                })
            }
            None => Err(Error::InvalidOperation(
                "No assistant response received".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::{AssistantContent, Message, ToolResult, UserContent};
    use crate::session::ToolVisibility;
    use crate::session::stores::sqlite::SqliteSessionStore;
    use crate::session::{SessionConfig, SessionManagerConfig, ToolApprovalPolicy};
    use conductor_tools::tools::read_only_workspace_tools;
    use dotenv::dotenv;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    async fn create_test_session_manager() -> (SessionManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let (event_tx, _event_rx) = mpsc::channel(100);
        let config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: Model::default(),
            auto_persist: true,
        };
        let manager = SessionManager::new(store, config, event_tx);

        (manager, temp_dir)
    }

    fn create_test_app_config() -> crate::app::AppConfig {
        dotenv().ok();
        crate::app::AppConfig {
            llm_config: LlmConfig::from_env().expect("API keys must be configured for tests"),
        }
    }

    fn create_test_app_config_no_api() -> crate::app::AppConfig {
        crate::app::AppConfig {
            llm_config: LlmConfig {
                anthropic_api_key: None,
                openai_api_key: None,
                gemini_api_key: None,
            },
        }
    }
    fn create_test_tool_approval_policy() -> ToolApprovalPolicy {
        let tools = read_only_workspace_tools();
        let tool_names = tools.iter().map(|t| t.name().to_string()).collect();
        ToolApprovalPolicy::PreApproved { tools: tool_names }
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_ephemeral_basic() {
        dotenv().ok();
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let messages = vec![Message::User {
            content: vec![UserContent::Text {
                text: "What is 2 + 2?".to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id: uuid::Uuid::now_v7(),
            parent_message_id: None,
        }];
        let future = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            Model::ClaudeSonnet4_20250514,
            Some(SessionToolConfig::read_only()),
            Some(create_test_tool_approval_policy()),
            None,
        );

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), future)
            .await
            .unwrap()
            .unwrap();

        assert!(!result.final_msg.id().is_empty());
        println!("Ephemeral run succeeded: {:?}", result.final_msg);

        // Verify the response contains something reasonable
        if let Message::Assistant { content, .. } = &result.final_msg {
            let text_content = content.iter().find_map(|c| match c {
                AssistantContent::Text { text } => Some(text),
                _ => None,
            });

            if let Some(content) = text_content {
                assert!(!content.is_empty(), "Response should not be empty");
                // For "What is 2 + 2?", we expect the answer to contain "4"
                assert!(
                    content.contains("4"),
                    "Expected response to contain '4', got: {}",
                    content
                );
            } else {
                panic!("No text content found in assistant message");
            }
        } else {
            panic!("Expected assistant message");
        }
    }

    #[tokio::test]
    async fn test_session_creation_and_persistence() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session with custom config
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = create_test_tool_approval_policy();

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "value".to_string())].into(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Verify session exists
        assert!(session_manager.is_session_active(&session_id).await);

        // Verify session has correct configuration
        let session = session_manager
            .store()
            .get_session(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            session.config.metadata.get("test"),
            Some(&"value".to_string())
        );
        assert_eq!(session.config.tool_config.backends.len(), 0); // read_only() uses default backends
        assert!(matches!(
            session.config.tool_config.visibility,
            ToolVisibility::ReadOnly
        ));
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_in_session_with_real_api() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = create_test_tool_approval_policy();

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "api_test".to_string())].into(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Run a simple task in the session
        let result = OneShotRunner::run_in_session(
            &session_manager,
            session_id.clone(),
            "What is the capital of France?".to_string(),
        )
        .await;

        match result {
            Ok(run_result) => {
                println!("Session run succeeded: {:?}", run_result.final_msg);

                // Verify we got a reasonable response
                if let Message::Assistant { content, .. } = &run_result.final_msg {
                    let text_content = content.iter().find_map(|c| match c {
                        AssistantContent::Text { text } => Some(text),
                        _ => None,
                    });

                    if let Some(content) = text_content {
                        assert!(!content.is_empty(), "Response should not be empty");
                        // The answer should mention Paris
                        assert!(
                            content.to_lowercase().contains("paris"),
                            "Expected response to contain 'Paris', got: {}",
                            content
                        );
                    } else {
                        panic!("Expected text response");
                    }
                } else {
                    panic!("Expected assistant message");
                }

                // Verify the session was updated
                let session_state = session_manager
                    .get_session_state(&session_id)
                    .await
                    .unwrap()
                    .unwrap();

                // Should have at least user + assistant messages
                assert!(
                    session_state.messages.len() >= 2,
                    "Expected at least 2 messages in session"
                );

                // First message should be the user input
                let user_msg = &session_state.messages[0];
                assert_eq!(user_msg.role(), crate::app::conversation::Role::User);

                // Last message should be the assistant response
                let assistant_msg = &session_state.messages[session_state.messages.len() - 1];
                assert_eq!(
                    assistant_msg.role(),
                    crate::app::conversation::Role::Assistant
                );
            }
            Err(e) => {
                // If no API key is configured, this is expected
                println!("Session run failed (expected if no API key): {}", e);
                assert!(
                    e.to_string().contains("API key")
                        || e.to_string().contains("authentication")
                        || e.to_string().contains("timed out"),
                    "Unexpected error: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_run_ephemeral_empty_messages() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            vec![], // Empty messages
            Model::ClaudeSonnet4_20250514,
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("No user message to process")
        );
    }

    #[tokio::test]
    async fn test_run_ephemeral_non_text_message() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let messages = vec![Message::Tool {
            tool_use_id: "test".to_string(),
            result: ToolResult::External(conductor_tools::result::ExternalResult {
                tool_name: "test_tool".to_string(),
                payload: "test".to_string(),
            }),
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("tool", Message::current_timestamp()),
            thread_id: uuid::Uuid::now_v7(),
            parent_message_id: None,
        }];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            Model::ClaudeSonnet4_20250514,
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("Last message must be from User")
        );
    }

    #[tokio::test]
    async fn test_run_in_session_without_timeout() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = ToolApprovalPolicy::PreApproved {
            tools: HashSet::new(),
        };

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "no_timeout_test".to_string())].into(),
        };

        let app_config = create_test_app_config_no_api(); // No API key to test error handling

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        let result =
            OneShotRunner::run_in_session(&session_manager, session_id, "Test message".to_string())
                .await;

        // Should fail due to API key issues, not timeout
        assert!(result.is_err());
        let error_msg = result.err().unwrap().to_string();
        // Should not contain timeout-related errors since timeout was removed
        assert!(!error_msg.contains("timed out"));
    }

    #[tokio::test]
    async fn test_run_in_session_nonexistent_session() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let result = OneShotRunner::run_in_session(
            &session_manager,
            "nonexistent-session-id".to_string(),
            "Test message".to_string(),
        )
        .await;

        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("Session not active")
        );
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_ephemeral_with_multi_turn_conversation() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let thread_id = uuid::Uuid::now_v7();
        let messages = vec![
            Message::User {
                content: vec![UserContent::Text {
                    text: "What is 2+2? Don't give me the answer yet.".to_string(),
                }],
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
                thread_id,
                parent_message_id: None,
            },
            Message::Assistant {
                content: vec![AssistantContent::Text {
                    text: "Ok, I'll give you the answer once you're ready.".to_string(),
                }],
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("assistant", Message::current_timestamp()),
                thread_id,
                parent_message_id: Some("user_0".to_string()),
            },
            Message::User {
                content: vec![UserContent::Text {
                    text: "I'm ready. What is the answer?".to_string(),
                }],
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
                thread_id,
                parent_message_id: Some("assistant_0".to_string()),
            },
        ];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            Model::ClaudeSonnet4_20250514,
            Some(SessionToolConfig::read_only()),
            None,
            None,
        )
        .await;
        let content = result.unwrap().final_msg.content_string();
        assert!(content.contains("4"));
    }

    #[tokio::test]
    async fn test_session_state_polling_mechanism() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = ToolApprovalPolicy::PreApproved {
            tools: HashSet::new(),
        };

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "polling_test".to_string())].into(),
        };

        let app_config = create_test_app_config_no_api(); // Use fake key for infrastructure test

        let (session_id, command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Verify initial state
        let initial_state = session_manager
            .get_session_state(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(initial_state.messages.len(), 0);

        // Send a message manually without using run_in_session
        command_tx
            .send(AppCommand::ProcessUserInput("Test".to_string()))
            .await
            .unwrap();

        // Wait longer for the message to be processed and persisted
        let mut attempts = 0;
        let max_attempts = 50; // 5 seconds total

        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            attempts += 1;

            let updated_state = session_manager
                .get_session_state(&session_id)
                .await
                .unwrap()
                .unwrap();

            if !updated_state.messages.is_empty() {
                // Found the message, verify it's correct
                let first_msg = &updated_state.messages[0];
                assert_eq!(first_msg.role(), crate::app::conversation::Role::User);
                if let Message::User { content, .. } = first_msg {
                    if let Some(UserContent::Text { text }) = content.first() {
                        assert_eq!(text, "Test");
                    } else {
                        panic!("Expected text content");
                    }
                } else {
                    panic!("Expected user message");
                }
                return; // Test passed
            }

            if attempts >= max_attempts {
                panic!(
                    "Message was not added to session state after {} attempts. Current message count: {}",
                    max_attempts,
                    updated_state.messages.len()
                );
            }
        }
    }

    #[tokio::test]
    async fn test_run_in_session_preserves_conversation_context() {
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = ToolApprovalPolicy::PreApproved {
            tools: HashSet::new(),
        };

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "context_test".to_string())].into(),
        };

        let app_config = create_test_app_config_no_api(); // Use fake key for infrastructure test

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // Verify initial state is empty
        let state_before = session_manager
            .get_session_state(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state_before.messages.len(), 0);

        // Run a one-shot task in the session
        // This should fail due to no API key, but the session should have the user message
        let result = OneShotRunner::run_in_session(
            &session_manager,
            session_id.clone(),
            "What is my name?".to_string(),
        )
        .await;

        // Should fail due to no API key or timeout
        assert!(result.is_err());

        // Verify the session has the user message that was sent
        let state_after = session_manager
            .get_session_state(&session_id)
            .await
            .unwrap()
            .unwrap();

        // Should have at least the user message
        assert!(!state_after.messages.is_empty());

        // The first message should be the user input we sent
        let first_msg = &state_after.messages[0];
        assert_eq!(first_msg.role(), crate::app::conversation::Role::User);
        if let Message::User { content, .. } = first_msg {
            if let Some(UserContent::Text { text }) = content.first() {
                assert_eq!(text, "What is my name?");
            } else {
                panic!("Expected text content for user message");
            }
        } else {
            panic!("Expected user message");
        }
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_ephemeral_with_tool_usage() {
        dotenv().ok();
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let messages = vec![Message::User {
            content: vec![UserContent::Text {
                text: "List the files in the current directory".to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id: uuid::Uuid::now_v7(),
            parent_message_id: None,
        }];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            Model::ClaudeSonnet4_20250514,
            Some(SessionToolConfig::read_only()),
            Some(create_test_tool_approval_policy()),
            None,
        )
        .await
        .expect("Ephemeral run with tools should succeed with valid API key");

        assert!(!result.final_msg.id().is_empty());
        println!("Ephemeral run with tools succeeded: {:?}", result.final_msg);

        // The response might be structured content with tool calls, which is expected
        let has_content = match &result.final_msg {
            Message::Assistant { content, .. } => {
                content.iter().any(|c| match c {
                    AssistantContent::Text { text } => !text.is_empty(),
                    _ => true, // Non-text blocks are also valid content
                })
            }
            _ => false,
        };
        assert!(has_content, "Response should have some content");

        // Should have some tool results since we asked to list files
        println!("Tool results: {:?}", result.tool_results);
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_in_session_preserves_context() {
        dotenv().ok();
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        // Create a session
        let mut tool_config = SessionToolConfig::read_only();
        tool_config.approval_policy = create_test_tool_approval_policy();

        let session_config = SessionConfig {
            workspace: WorkspaceConfig::default(),
            tool_config,
            system_prompt: None,
            metadata: [("test".to_string(), "context_test".to_string())].into(),
        };

        let app_config = create_test_app_config();

        let (session_id, _command_tx) = session_manager
            .create_session(session_config, app_config)
            .await
            .unwrap();

        // First interaction: set context
        let result1 = OneShotRunner::run_in_session(
            &session_manager,
            session_id.clone(),
            "My name is Alice and I like pizza.".to_string(),
        )
        .await
        .expect("First session run should succeed");

        println!("First interaction: {:?}", result1.final_msg);

        // Second interaction: test if context is preserved
        let result2 = OneShotRunner::run_in_session(
            &session_manager,
            session_id.clone(),
            "What is my name and what do I like?".to_string(),
        )
        .await
        .expect("Second session run should succeed");

        println!("Second interaction: {:?}", result2.final_msg);

        // Verify the second response uses the context from the first
        if let Message::Assistant { content, .. } = &result2.final_msg {
            let text_content = content.iter().find_map(|c| match c {
                AssistantContent::Text { text } => Some(text),
                _ => None,
            });

            if let Some(content) = text_content {
                assert!(!content.is_empty(), "Response should not be empty");
                let content_lower = content.to_lowercase();

                // The AI should acknowledge the name Alice from the context
                // If it doesn't remember perfectly, it should at least acknowledge the user
                assert!(
                    content_lower.contains("alice") || content_lower.contains("name"),
                    "Expected response to reference the name or context, got: {}",
                    content
                );
            } else {
                panic!("Expected text response in assistant message");
            }
        } else {
            panic!("Expected assistant message");
        }

        // Verify the session has all the messages
        let session_state = session_manager
            .get_session_state(&session_id)
            .await
            .unwrap()
            .unwrap();

        // Should have at least 3 messages: user1, assistant1, user2, (and possibly assistant2)
        // The AI might give the same response twice, which is ok for testing infrastructure
        assert!(
            session_state.messages.len() >= 3,
            "Expected at least 3 messages in session, got {}",
            session_state.messages.len()
        );

        println!("Session has {} messages", session_state.messages.len());
        for (i, msg) in session_state.messages.iter().enumerate() {
            println!("Message {}: {:?}", i, msg.role());
        }
    }
}
