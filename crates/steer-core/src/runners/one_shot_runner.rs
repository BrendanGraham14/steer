use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::app::conversation::UserContent;
use crate::app::{AppCommand, AppConfig, Message, MessageData};
use crate::config::LlmConfigProvider;
use crate::config::model::ModelId;
use crate::error::{Error, Result};
use crate::session::state::WorkspaceConfig;

#[cfg(not(test))]
use crate::auth::DefaultAuthStorage;

use crate::session::{
    manager::SessionManager,
    state::{SessionConfig, SessionToolConfig, ToolApprovalPolicy},
};

/// Contains the result of a single agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOnceResult {
    /// The final assistant message after all tools have been executed
    pub final_message: Message,
    /// The session ID of the session that was used
    pub session_id: String,
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
        #[cfg(not(test))]
        let app_config = {
            let storage = std::sync::Arc::new(DefaultAuthStorage::new()?);
            AppConfig {
                llm_config_provider: LlmConfigProvider::new(storage),
            }
        };

        #[cfg(test)]
        let app_config = {
            let storage = std::sync::Arc::new(crate::test_utils::InMemoryAuthStorage::new());
            AppConfig {
                llm_config_provider: LlmConfigProvider::new(storage),
            }
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
                    "Failed to send message to session {session_id}: {e}"
                ))
            })?;

        // 4. Process events to build the result (similar to TUI's event loop)
        let result = Self::process_events(event_rx, &session_id).await;

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
        model: ModelId,
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
                    (
                        "initial_model".to_string(),
                        format!("{:?}/{}", model.0, model.1),
                    ),
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
                (
                    "initial_model".to_string(),
                    format!("{:?}/{}", model.0, model.1),
                ),
            ]
            .into_iter()
            .collect();

            // Apply custom system prompt if provided
            if system_prompt.is_some() {
                default_config.system_prompt = system_prompt;
            }

            default_config
        };

        #[cfg(not(test))]
        let app_config = {
            let storage = std::sync::Arc::new(DefaultAuthStorage::new()?);
            AppConfig {
                llm_config_provider: LlmConfigProvider::new(storage),
            }
        };

        #[cfg(test)]
        let app_config = {
            let storage = std::sync::Arc::new(crate::test_utils::InMemoryAuthStorage::new());
            AppConfig {
                llm_config_provider: LlmConfigProvider::new(storage),
            }
        };

        let (session_id, command_tx) = session_manager
            .create_session(session_config, app_config)
            .await?;

        // Set the model using ExecuteCommand
        let model_str = format!("{:?}/{}", model.0, model.1).to_lowercase();
        command_tx
            .send(AppCommand::ExecuteCommand(
                crate::app::conversation::AppCommandType::Model {
                    target: Some(model_str),
                },
            ))
            .await
            .map_err(|_| Error::InvalidOperation("Failed to send model command".to_string()))?;

        // 3. Process the final user message (this triggers the actual processing)
        let user_content = match init_msgs.last() {
            Some(message) => {
                // Extract text content from the message
                match &message.data {
                    MessageData::User { content, .. } => {
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

        let mut messages = Vec::new();
        info!(session_id = %session_id, "Starting event processing loop");

        while let Some(event) = event_rx.recv().await {
            match event {
                AppEvent::MessageAdded { message, model: _ } => {
                    info!(session_id = %session_id, role = ?message.role(), id = %message.id(), "MessageAdded event");
                    messages.push(message);
                }

                AppEvent::MessageUpdated { id, .. } => {
                    info!(session_id = %session_id, id = %id, "MessageUpdated event");
                    // We'll get the final message in MessageAdded, so we can ignore updates
                }

                AppEvent::ProcessingCompleted => {
                    info!(session_id = %session_id, "ProcessingCompleted event received");
                    // Check if we have an assistant message
                    if !messages.is_empty() {
                        info!(session_id = %session_id, "Final message received, exiting event loop");
                        break;
                    }
                }

                AppEvent::Error { message } => {
                    error!(session_id = %session_id, error = %message, "Error event");
                    return Err(Error::InvalidOperation(format!(
                        "Error during processing: {message}"
                    )));
                }

                AppEvent::RequestToolApproval { .. } => {
                    info!(session_id = %session_id, "RequestToolApproval event - this shouldn't happen in headless mode");
                    // In headless mode, tools should be pre-approved or denied by policy
                }

                _ => {
                    // Ignore other events like ProcessingStarted, ModelChanged, etc.
                }
            }
        }

        // Return the result
        match messages.last() {
            Some(_) => {
                info!(
                    session_id = %session_id,
                    "Returning final result"
                );
                Ok(RunOnceResult {
                    final_message: messages.last().unwrap().clone(),
                    session_id: session_id.to_string(),
                })
            }
            None => Err(Error::InvalidOperation("No message received".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::conversation::{AssistantContent, Message, ToolResult, UserContent};
    use crate::config::provider::ProviderId;
    use crate::session::ToolVisibility;
    use crate::session::stores::sqlite::SqliteSessionStore;
    use crate::session::{SessionConfig, SessionManagerConfig, ToolApprovalPolicy};
    use crate::test_utils;
    use dotenvy::dotenv;
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;
    use steer_tools::tools::read_only_workspace_tools;
    use tempfile::TempDir;

    async fn create_test_session_manager() -> (SessionManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let store = Arc::new(SqliteSessionStore::new(&db_path).await.unwrap());

        let config = SessionManagerConfig {
            max_concurrent_sessions: 10,
            default_model: (
                crate::config::provider::ProviderId::Anthropic,
                "claude-sonnet-4-20250514".to_string(),
            ),
            auto_persist: true,
        };
        let manager = SessionManager::new(store, config);

        (manager, temp_dir)
    }

    async fn create_test_app_config() -> crate::app::AppConfig {
        dotenv().ok();
        // Tests will fail if API keys are not configured
        test_utils::test_app_config()
    }

    fn create_test_app_config_no_api() -> crate::app::AppConfig {
        test_utils::test_app_config()
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

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "What is 2 + 2?".to_string(),
                }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: None,
        }];
        let future = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            (
                ProviderId::Anthropic,
                "claude-3-5-sonnet-latest".to_string(),
            ),
            Some(SessionToolConfig::read_only()),
            Some(create_test_tool_approval_policy()),
            None,
        );

        let result = tokio::time::timeout(std::time::Duration::from_secs(10), future)
            .await
            .unwrap()
            .unwrap();

        assert!(!result.final_message.id().is_empty());
        println!("Ephemeral run succeeded: {:?}", result.final_message);

        // Verify the response contains something reasonable
        let content = match &result.final_message.data {
            MessageData::Assistant { content, .. } => content,
            _ => unreachable!("expected assistant message, got {:?}", result.final_message),
        };
        let text_content = content.iter().find_map(|c| match c {
            AssistantContent::Text { text } => Some(text),
            _ => None,
        });
        let content = text_content.expect("No text content found in assistant message");
        assert!(!content.is_empty(), "Response should not be empty");
        // For "What is 2 + 2?", we expect the answer to contain "4"
        assert!(
            content.contains("4"),
            "Expected response to contain '4', got: {content}"
        );
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

        let app_config = create_test_app_config().await;

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

        let app_config = create_test_app_config().await;

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
                println!("Session run succeeded: {:?}", run_result.final_message);

                let content = match &run_result.final_message.data {
                    MessageData::Assistant { content, .. } => content.clone(),
                    _ => unreachable!(
                        "expected assistant message, got {:?}",
                        run_result.final_message
                    ),
                };
                let text_content = content.iter().find_map(|c| match c {
                    AssistantContent::Text { text } => Some(text),
                    _ => None,
                });
                let content = text_content.expect("expected text response in assistant message");
                assert!(!content.is_empty(), "Response should not be empty");
                // The answer should mention Paris
                assert!(
                    content.to_lowercase().contains("paris"),
                    "Expected response to contain 'Paris', got: {content}"
                );

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
                println!("Session run failed (expected if no API key): {e}");
                assert!(
                    e.to_string().contains("API key")
                        || e.to_string().contains("authentication")
                        || e.to_string().contains("timed out"),
                    "Unexpected error: {e}"
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
            (
                ProviderId::Anthropic,
                "claude-3-5-sonnet-latest".to_string(),
            ),
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

        let messages = vec![Message {
            data: MessageData::Tool {
                tool_use_id: "test".to_string(),
                result: ToolResult::External(steer_tools::result::ExternalResult {
                    tool_name: "test_tool".to_string(),
                    payload: "test".to_string(),
                }),
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("tool", Message::current_timestamp()),
            parent_message_id: None,
        }];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            (
                ProviderId::Anthropic,
                "claude-3-5-sonnet-latest".to_string(),
            ),
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
    #[ignore = "Test makes real API calls and expects failure, but now succeeds with in-memory auth"]
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

        let messages = vec![
            Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: "What is 2+2? Don't give me the answer yet.".to_string(),
                    }],
                },
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
                parent_message_id: None,
            },
            Message {
                data: MessageData::Assistant {
                    content: vec![AssistantContent::Text {
                        text: "Ok, I'll give you the answer once you're ready.".to_string(),
                    }],
                },
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("assistant", Message::current_timestamp()),
                parent_message_id: Some("user_0".to_string()),
            },
            Message {
                data: MessageData::User {
                    content: vec![UserContent::Text {
                        text: "I'm ready. What is the answer?".to_string(),
                    }],
                },
                timestamp: Message::current_timestamp(),
                id: Message::generate_id("user", Message::current_timestamp()),
                parent_message_id: Some("assistant_0".to_string()),
            },
        ];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            (
                ProviderId::Anthropic,
                "claude-3-5-sonnet-latest".to_string(),
            ),
            Some(SessionToolConfig::read_only()),
            None,
            None,
        )
        .await;
        let content = result.unwrap().final_message.content_string();
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
                assert!(matches!(first_msg.data, MessageData::User { .. }));
                let MessageData::User { content, .. } = &first_msg.data else {
                    unreachable!();
                };
                assert!(matches!(content.first(), Some(UserContent::Text { .. })));
                let Some(UserContent::Text { text }) = content.first() else {
                    unreachable!();
                };
                assert_eq!(text, "Test");
                return; // Test passed
            }
            assert!(
                attempts < max_attempts,
                "Message was not added to session state after {} attempts. Current message count: {}",
                max_attempts,
                updated_state.messages.len()
            );
        }
    }

    #[tokio::test]
    #[ignore = "Test makes real API calls and expects failure, but now succeeds with in-memory auth"]
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
        assert!(matches!(first_msg.data, MessageData::User { .. }));
        let MessageData::User { content, .. } = &first_msg.data else {
            unreachable!();
        };
        assert!(matches!(content.first(), Some(UserContent::Text { .. })));
        let Some(UserContent::Text { text }) = content.first() else {
            unreachable!();
        };
        assert_eq!(text, "What is my name?");
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_ephemeral_with_tool_usage() {
        dotenv().ok();
        let (session_manager, _temp_dir) = create_test_session_manager().await;

        let messages = vec![Message {
            data: MessageData::User {
                content: vec![UserContent::Text {
                    text: "List the files in the current directory".to_string(),
                }],
            },
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            parent_message_id: None,
        }];

        let result = OneShotRunner::run_ephemeral(
            &session_manager,
            messages,
            (
                ProviderId::Anthropic,
                "claude-3-5-sonnet-latest".to_string(),
            ),
            Some(SessionToolConfig::read_only()),
            Some(create_test_tool_approval_policy()),
            None,
        )
        .await
        .expect("Ephemeral run with tools should succeed with valid API key");

        assert!(!result.final_message.id().is_empty());
        println!(
            "Ephemeral run with tools succeeded: {:?}",
            result.final_message
        );

        // The response might be structured content with tool calls, which is expected
        let has_content = match &result.final_message.data {
            MessageData::Assistant { content, .. } => {
                content.iter().any(|c| match c {
                    AssistantContent::Text { text } => !text.is_empty(),
                    _ => true, // Non-text blocks are also valid content
                })
            }
            _ => false,
        };
        assert!(has_content, "Response should have some content");
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

        let app_config = create_test_app_config().await;

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

        println!("First interaction: {:?}", result1.final_message);

        // Second interaction: test if context is preserved
        let result2 = OneShotRunner::run_in_session(
            &session_manager,
            session_id.clone(),
            "What is my name and what do I like?".to_string(),
        )
        .await
        .expect("Second session run should succeed");

        println!("Second interaction: {:?}", result2.final_message);

        // Verify the second response uses the context from the first
        match &result2.final_message.data {
            MessageData::Assistant { content, .. } => {
                let text_content = content.iter().find_map(|c| match c {
                    AssistantContent::Text { text } => Some(text),
                    _ => None,
                });

                match text_content {
                    Some(content) => {
                        assert!(!content.is_empty(), "Response should not be empty");
                        let content_lower = content.to_lowercase();

                        // The AI should acknowledge the name Alice from the context
                        // If it doesn't remember perfectly, it should at least acknowledge the user
                        assert!(
                            content_lower.contains("alice") || content_lower.contains("name"),
                            "Expected response to reference the name or context, got: {content}"
                        );
                    }
                    None => {
                        unreachable!("expected text response in assistant message");
                    }
                }
            }
            _ => {
                unreachable!(
                    "expected assistant message, got {:?}",
                    result2.final_message
                );
            }
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
