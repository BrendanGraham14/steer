use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agents::default_agent_spec_id;
use crate::app::conversation::Message;
use crate::app::domain::event::SessionEvent;
use crate::app::domain::runtime::{RuntimeError, RuntimeHandle};
use crate::app::domain::types::SessionId;
use crate::config::model::ModelId;
use crate::error::{Error, Result};
use crate::session::ToolApprovalPolicy;
use crate::session::state::SessionConfig;
use crate::tools::{DISPATCH_AGENT_TOOL_NAME, DispatchAgentParams, DispatchAgentTarget};
use steer_tools::ToolCall;
use steer_tools::tools::BASH_TOOL_NAME;
use steer_tools::tools::bash::BashParams;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunOnceResult {
    pub final_message: Message,
    pub session_id: SessionId,
}

pub struct OneShotRunner;

impl Default for OneShotRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl OneShotRunner {
    pub fn new() -> Self {
        Self
    }

    pub async fn run_in_session(
        runtime: &RuntimeHandle,
        session_id: SessionId,
        message: String,
        model: ModelId,
    ) -> Result<RunOnceResult> {
        Self::run_in_session_with_cancel(
            runtime,
            session_id,
            message,
            model,
            CancellationToken::new(),
        )
        .await
    }

    pub async fn run_in_session_with_cancel(
        runtime: &RuntimeHandle,
        session_id: SessionId,
        message: String,
        model: ModelId,
        cancel_token: CancellationToken,
    ) -> Result<RunOnceResult> {
        runtime.resume_session(session_id).await.map_err(|e| {
            Error::InvalidOperation(format!("Failed to resume session {session_id}: {e}"))
        })?;

        let subscription = runtime.subscribe_events(session_id).await.map_err(|e| {
            Error::InvalidOperation(format!(
                "Failed to subscribe to session {session_id} events: {e}"
            ))
        })?;

        let approval_policy = match runtime.get_session_state(session_id).await {
            Ok(state) => state
                .session_config
                .map(|config| config.tool_config.approval_policy)
                .unwrap_or_default(),
            Err(err) => {
                warn!(
                    session_id = %session_id,
                    error = %err,
                    "Failed to load session approval policy; defaulting to deny"
                );
                ToolApprovalPolicy::default()
            }
        };

        info!(session_id = %session_id, message = %message, "Sending message to session");

        let op_id = runtime
            .submit_user_input(session_id, message, model)
            .await
            .map_err(|e| {
                Error::InvalidOperation(format!(
                    "Failed to send message to session {session_id}: {e}"
                ))
            })?;

        let cancel_task = {
            let runtime = runtime.clone();
            let cancel_token = cancel_token.clone();
            tokio::spawn(async move {
                cancel_token.cancelled().await;
                if let Err(err) = runtime.cancel_operation(session_id, Some(op_id)).await {
                    warn!(
                        session_id = %session_id,
                        error = %err,
                        "Failed to cancel one-shot operation"
                    );
                }
            })
        };

        let result =
            Self::process_events(runtime, subscription, session_id, op_id, approval_policy).await;

        cancel_task.abort();

        if let Err(e) = runtime.suspend_session(session_id).await {
            error!(session_id = %session_id, error = %e, "Failed to suspend session");
        } else {
            info!(session_id = %session_id, "Session suspended successfully");
        }

        result
    }

    pub async fn run_new_session(
        runtime: &RuntimeHandle,
        config: SessionConfig,
        message: String,
        model: ModelId,
    ) -> Result<RunOnceResult> {
        Self::run_new_session_with_cancel(runtime, config, message, model, CancellationToken::new())
            .await
    }

    pub async fn run_new_session_with_cancel(
        runtime: &RuntimeHandle,
        config: SessionConfig,
        message: String,
        model: ModelId,
        cancel_token: CancellationToken,
    ) -> Result<RunOnceResult> {
        let session_id = runtime
            .create_session(config)
            .await
            .map_err(|e| Error::InvalidOperation(format!("Failed to create session: {e}")))?;

        info!(session_id = %session_id, "Created new session for one-shot run");

        Self::run_in_session_with_cancel(runtime, session_id, message, model, cancel_token).await
    }

    async fn process_events(
        runtime: &RuntimeHandle,
        mut subscription: crate::app::domain::runtime::SessionEventSubscription,
        session_id: SessionId,
        op_id: crate::app::domain::types::OpId,
        approval_policy: ToolApprovalPolicy,
    ) -> Result<RunOnceResult> {
        let mut messages = Vec::new();
        info!(session_id = %session_id, "Starting event processing loop");

        while let Some(envelope) = subscription.recv().await {
            match envelope.event {
                SessionEvent::AssistantMessageAdded { message, model: _ } => {
                    info!(
                        session_id = %session_id,
                        role = ?message.role(),
                        id = %message.id(),
                        "AssistantMessageAdded event"
                    );
                    messages.push(message);
                }

                SessionEvent::MessageUpdated { message } => {
                    info!(
                        session_id = %session_id,
                        id = %message.id(),
                        "MessageUpdated event"
                    );
                }

                SessionEvent::OperationCompleted {
                    op_id: completed_op,
                } => {
                    if completed_op != op_id {
                        continue;
                    }
                    info!(
                        session_id = %session_id,
                        op_id = %completed_op,
                        "OperationCompleted event received"
                    );
                    if !messages.is_empty() {
                        info!(session_id = %session_id, "Final message received, exiting event loop");
                        break;
                    }
                }

                SessionEvent::OperationCancelled {
                    op_id: cancelled_op,
                    ..
                } => {
                    if cancelled_op != op_id {
                        continue;
                    }
                    warn!(
                        session_id = %session_id,
                        op_id = %cancelled_op,
                        "OperationCancelled event received"
                    );
                    return Err(Error::Cancelled);
                }

                SessionEvent::Error { message } => {
                    error!(session_id = %session_id, error = %message, "Error event");
                    return Err(Error::InvalidOperation(format!(
                        "Error during processing: {message}"
                    )));
                }

                SessionEvent::ApprovalRequested {
                    request_id,
                    tool_call,
                } => {
                    let approved = tool_is_preapproved(&tool_call, &approval_policy);
                    if approved {
                        info!(
                            session_id = %session_id,
                            request_id = %request_id,
                            tool = %tool_call.name,
                            "Auto-approving preapproved tool"
                        );
                    } else {
                        warn!(
                            session_id = %session_id,
                            request_id = %request_id,
                            tool = %tool_call.name,
                            "Auto-denying unapproved tool"
                        );
                    }

                    runtime
                        .submit_tool_approval(session_id, request_id, approved, None)
                        .await
                        .map_err(|e| {
                            Error::InvalidOperation(format!(
                                "Failed to submit tool approval decision: {e}"
                            ))
                        })?;
                }

                _ => {}
            }
        }

        match messages.last() {
            Some(final_message) => {
                info!(
                    session_id = %session_id,
                    message_count = messages.len(),
                    "Returning final result"
                );
                Ok(RunOnceResult {
                    final_message: final_message.clone(),
                    session_id,
                })
            }
            None => Err(Error::InvalidOperation("No message received".to_string())),
        }
    }
}

fn tool_is_preapproved(tool_call: &ToolCall, policy: &ToolApprovalPolicy) -> bool {
    if policy.preapproved.tools.contains(&tool_call.name) {
        return true;
    }

    if tool_call.name == DISPATCH_AGENT_TOOL_NAME {
        let params = serde_json::from_value::<DispatchAgentParams>(tool_call.parameters.clone());
        if let Ok(params) = params {
            return match params.target {
                DispatchAgentTarget::Resume { .. } => true,
                DispatchAgentTarget::New { agent, .. } => {
                    let agent_id = agent
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| default_agent_spec_id().to_string());
                    policy.is_dispatch_agent_pattern_preapproved(&agent_id)
                }
            };
        }
    }

    if tool_call.name == BASH_TOOL_NAME {
        let params = serde_json::from_value::<BashParams>(tool_call.parameters.clone());
        if let Ok(params) = params {
            return policy.is_bash_pattern_preapproved(&params.command);
        }
    }

    false
}

impl From<RuntimeError> for Error {
    fn from(e: RuntimeError) -> Self {
        match e {
            RuntimeError::SessionNotFound { session_id } => {
                Error::InvalidOperation(format!("Session not found: {session_id}"))
            }
            RuntimeError::SessionAlreadyExists { session_id } => {
                Error::InvalidOperation(format!("Session already exists: {session_id}"))
            }
            RuntimeError::InvalidInput { message } => Error::InvalidOperation(message),
            RuntimeError::ChannelClosed => {
                Error::InvalidOperation("Runtime channel closed".to_string())
            }
            RuntimeError::ShuttingDown => {
                Error::InvalidOperation("Runtime is shutting down".to_string())
            }
            RuntimeError::Session(e) => Error::InvalidOperation(format!("Session error: {e}")),
            RuntimeError::EventStore(e) => {
                Error::InvalidOperation(format!("Event store error: {e}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client as ApiClient;
    use crate::api::{ApiError, CompletionResponse, Provider};
    use crate::app::conversation::{AssistantContent, Message, MessageData};
    use crate::app::domain::action::ApprovalDecision;
    use crate::app::domain::runtime::RuntimeService;
    use crate::app::domain::session::event_store::InMemoryEventStore;
    use crate::app::validation::ValidatorRegistry;
    use crate::config::model::builtin;
    use crate::session::ToolApprovalPolicy;
    use crate::session::state::{
        ApprovalRules, SessionToolConfig, UnapprovedBehavior, WorkspaceConfig,
    };
    use crate::tools::static_tools::READ_ONLY_TOOL_NAMES;
    use crate::tools::{BackendRegistry, ToolExecutor};
    use dotenvy::dotenv;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use steer_tools::ToolCall;
    use steer_tools::tools::BASH_TOOL_NAME;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct ToolCallThenTextProvider {
        tool_call: ToolCall,
        final_text: String,
        call_count: Arc<StdMutex<usize>>,
    }

    impl ToolCallThenTextProvider {
        fn new(tool_call: ToolCall, final_text: impl Into<String>) -> Self {
            Self {
                tool_call,
                final_text: final_text.into(),
                call_count: Arc::new(StdMutex::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl Provider for ToolCallThenTextProvider {
        fn name(&self) -> &'static str {
            "stub-tool-call"
        }

        async fn complete(
            &self,
            _model_id: &crate::config::model::ModelId,
            _messages: Vec<Message>,
            _system: Option<crate::app::SystemContext>,
            _tools: Option<Vec<steer_tools::ToolSchema>>,
            _call_options: Option<crate::config::model::ModelParameters>,
            _token: CancellationToken,
        ) -> std::result::Result<CompletionResponse, ApiError> {
            let mut count = self
                .call_count
                .lock()
                .expect("tool call counter lock poisoned");
            let response = if *count == 0 {
                CompletionResponse {
                    content: vec![AssistantContent::ToolCall {
                        tool_call: self.tool_call.clone(),
                        thought_signature: None,
                    }],
                }
            } else {
                CompletionResponse {
                    content: vec![AssistantContent::Text {
                        text: self.final_text.clone(),
                    }],
                }
            };
            *count += 1;
            Ok(response)
        }
    }

    async fn create_test_runtime() -> RuntimeService {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(crate::model_registry::ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry,
        ));

        let tool_executor = Arc::new(ToolExecutor::with_components(
            Arc::new(BackendRegistry::new()),
            Arc::new(ValidatorRegistry::new()),
        ));

        RuntimeService::spawn(event_store, api_client, tool_executor)
    }

    fn create_test_session_config() -> SessionConfig {
        SessionConfig {
            default_model: builtin::claude_sonnet_4_5(),
            workspace: WorkspaceConfig::default(),
            workspace_ref: None,
            workspace_id: None,
            repo_ref: None,
            parent_session_id: None,
            workspace_name: None,
            tool_config: SessionToolConfig::default(),
            system_prompt: None,
            metadata: std::collections::HashMap::new(),
        }
    }

    fn create_test_tool_approval_policy() -> ToolApprovalPolicy {
        let tool_names = READ_ONLY_TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect();
        ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: tool_names,
                per_tool: std::collections::HashMap::new(),
            },
        }
    }

    #[test]
    fn tool_is_preapproved_allows_whitelisted_tool() {
        let policy = create_test_tool_approval_policy();
        let tool_call = ToolCall {
            id: "tc_read".to_string(),
            name: READ_ONLY_TOOL_NAMES[0].to_string(),
            parameters: json!({}),
        };

        assert!(tool_is_preapproved(&tool_call, &policy));
    }

    #[test]
    fn tool_is_preapproved_allows_bash_pattern() {
        use crate::session::state::{ApprovalRules, ToolRule, UnapprovedBehavior};

        let mut per_tool = HashMap::new();
        per_tool.insert(
            "bash".to_string(),
            ToolRule::Bash {
                patterns: vec!["echo *".to_string()],
            },
        );

        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: HashSet::new(),
                per_tool,
            },
        };

        let tool_call = ToolCall {
            id: "tc_bash".to_string(),
            name: BASH_TOOL_NAME.to_string(),
            parameters: json!({ "command": "echo hello" }),
        };

        assert!(tool_is_preapproved(&tool_call, &policy));
    }

    #[test]
    fn tool_is_preapproved_allows_dispatch_agent_pattern() {
        use crate::session::state::{ApprovalRules, ToolRule, UnapprovedBehavior};

        let mut per_tool = HashMap::new();
        per_tool.insert(
            "dispatch_agent".to_string(),
            ToolRule::DispatchAgent {
                agent_patterns: vec!["explore".to_string()],
            },
        );

        let policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: HashSet::new(),
                per_tool,
            },
        };

        let tool_call = ToolCall {
            id: "tc_dispatch".to_string(),
            name: DISPATCH_AGENT_TOOL_NAME.to_string(),
            parameters: json!({
                "prompt": "find files",
                "target": {
                    "session": "new",
                    "workspace": {
                        "location": "current"
                    },
                    "agent": "explore"
                }
            }),
        };

        assert!(tool_is_preapproved(&tool_call, &policy));
    }

    #[test]
    fn tool_is_preapproved_denies_unlisted_tool() {
        let policy = create_test_tool_approval_policy();
        let tool_call = ToolCall {
            id: "tc_other".to_string(),
            name: "bash".to_string(),
            parameters: json!({ "command": "rm -rf /" }),
        };

        assert!(!tool_is_preapproved(&tool_call, &policy));
    }

    #[tokio::test]
    async fn run_new_session_denies_unapproved_tool_requests() {
        let event_store = Arc::new(InMemoryEventStore::new());
        let model_registry = Arc::new(crate::model_registry::ModelRegistry::load(&[]).unwrap());
        let provider_registry = Arc::new(crate::auth::ProviderRegistry::load(&[]).unwrap());
        let api_client = Arc::new(ApiClient::new_with_deps(
            crate::test_utils::test_llm_config_provider(),
            provider_registry,
            model_registry.clone(),
        ));

        let tool_call = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            parameters: json!({ "command": "echo denied" }),
        };
        api_client.insert_test_provider(
            builtin::claude_sonnet_4_5().provider.clone(),
            Arc::new(ToolCallThenTextProvider::new(tool_call, "done")),
        );

        let tool_executor = Arc::new(ToolExecutor::with_components(
            Arc::new(BackendRegistry::new()),
            Arc::new(ValidatorRegistry::new()),
        ));
        let runtime = RuntimeService::spawn(event_store, api_client, tool_executor);

        let mut config = create_test_session_config();
        config.tool_config.approval_policy = ToolApprovalPolicy {
            default_behavior: UnapprovedBehavior::Prompt,
            preapproved: ApprovalRules {
                tools: HashSet::new(),
                per_tool: HashMap::new(),
            },
        };

        let model = builtin::claude_sonnet_4_5();
        let result = OneShotRunner::run_new_session(
            &runtime.handle,
            config,
            "Trigger tool call".to_string(),
            model,
        )
        .await
        .expect("run_new_session should complete");

        let events = runtime
            .handle
            .load_events_after(result.session_id, 0)
            .await
            .expect("load events");

        let mut saw_request = false;
        let mut saw_decision = false;
        let mut saw_denied = false;

        for (_, event) in events {
            match event {
                SessionEvent::ApprovalRequested { .. } => saw_request = true,
                SessionEvent::ApprovalDecided { decision, .. } => {
                    saw_decision = true;
                    if decision == ApprovalDecision::Denied {
                        saw_denied = true;
                    }
                }
                _ => {}
            }
        }

        assert!(saw_request, "expected ApprovalRequested event");
        assert!(saw_decision, "expected ApprovalDecided event");
        assert!(saw_denied, "expected denied decision");

        runtime.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_new_session_basic() {
        dotenv().ok();
        let runtime = create_test_runtime().await;

        let mut config = create_test_session_config();
        config.tool_config = SessionToolConfig::read_only();
        config.tool_config.approval_policy = create_test_tool_approval_policy();
        config
            .metadata
            .insert("mode".to_string(), "headless".to_string());

        let model = builtin::claude_sonnet_4_5();
        let result = OneShotRunner::run_new_session(
            &runtime.handle,
            config,
            "What is 2 + 2?".to_string(),
            model,
        )
        .await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(30), async { result })
            .await
            .expect("Timed out waiting for response")
            .expect("run_new_session failed");

        assert!(!result.final_message.id().is_empty());
        println!("New session run succeeded: {:?}", result.final_message);

        let content = match &result.final_message.data {
            MessageData::Assistant { content, .. } => content,
            _ => panic!("expected assistant message, got {:?}", result.final_message),
        };
        let text_content = content.iter().find_map(|c| match c {
            AssistantContent::Text { text } => Some(text),
            _ => None,
        });
        let content = text_content.expect("No text content found in assistant message");
        assert!(!content.is_empty(), "Response should not be empty");
        assert!(
            content.contains("4"),
            "Expected response to contain '4', got: {content}"
        );

        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn test_session_creation() {
        let runtime = create_test_runtime().await;

        let mut config = create_test_session_config();
        config.tool_config.approval_policy = create_test_tool_approval_policy();
        config
            .metadata
            .insert("test".to_string(), "value".to_string());

        let session_id = runtime.handle.create_session(config).await.unwrap();

        assert!(runtime.handle.is_session_active(session_id).await.unwrap());

        let state = runtime.handle.get_session_state(session_id).await.unwrap();
        assert_eq!(
            state.session_config.as_ref().unwrap().metadata.get("test"),
            Some(&"value".to_string())
        );

        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn test_run_in_session_nonexistent_session() {
        let runtime = create_test_runtime().await;

        let fake_session_id = SessionId::new();
        let model = builtin::claude_sonnet_4_5();
        let result = OneShotRunner::run_in_session(
            &runtime.handle,
            fake_session_id,
            "Test message".to_string(),
            model,
        )
        .await;

        assert!(result.is_err());
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("not found") || err.contains("Session"),
            "Expected session not found error, got: {err}"
        );

        runtime.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_in_session_with_real_api() {
        dotenv().ok();
        let runtime = create_test_runtime().await;

        let mut config = create_test_session_config();
        config.tool_config = SessionToolConfig::read_only();
        config.tool_config.approval_policy = create_test_tool_approval_policy();
        config
            .metadata
            .insert("test".to_string(), "api_test".to_string());

        let session_id = runtime.handle.create_session(config).await.unwrap();
        let model = builtin::claude_sonnet_4_5();

        let result = OneShotRunner::run_in_session(
            &runtime.handle,
            session_id,
            "What is the capital of France?".to_string(),
            model,
        )
        .await;

        match result {
            Ok(run_result) => {
                println!("Session run succeeded: {:?}", run_result.final_message);

                let content = match &run_result.final_message.data {
                    MessageData::Assistant { content, .. } => content.clone(),
                    _ => panic!(
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
                assert!(
                    content.to_lowercase().contains("paris"),
                    "Expected response to contain 'Paris', got: {content}"
                );
            }
            Err(e) => {
                println!("Session run failed (expected if no API key): {e}");
                assert!(
                    e.to_string().contains("API key")
                        || e.to_string().contains("authentication")
                        || e.to_string().contains("timed out"),
                    "Unexpected error: {e}"
                );
            }
        }

        runtime.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_in_session_preserves_context() {
        dotenv().ok();
        let runtime = create_test_runtime().await;

        let mut config = create_test_session_config();
        config.tool_config = SessionToolConfig::read_only();
        config.tool_config.approval_policy = create_test_tool_approval_policy();
        config
            .metadata
            .insert("test".to_string(), "context_test".to_string());

        let session_id = runtime.handle.create_session(config).await.unwrap();
        let model = builtin::claude_sonnet_4_5();

        let result1 = OneShotRunner::run_in_session(
            &runtime.handle,
            session_id,
            "My name is Alice and I like pizza.".to_string(),
            model.clone(),
        )
        .await
        .expect("First session run should succeed");

        println!("First interaction: {:?}", result1.final_message);

        runtime.handle.resume_session(session_id).await.unwrap();

        let result2 = OneShotRunner::run_in_session(
            &runtime.handle,
            session_id,
            "What is my name and what do I like?".to_string(),
            model,
        )
        .await
        .expect("Second session run should succeed");

        println!("Second interaction: {:?}", result2.final_message);

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

                        assert!(
                            content_lower.contains("alice") || content_lower.contains("name"),
                            "Expected response to reference the name or context, got: {content}"
                        );
                    }
                    None => {
                        panic!("expected text response in assistant message");
                    }
                }
            }
            _ => {
                panic!(
                    "expected assistant message, got {:?}",
                    result2.final_message
                );
            }
        }

        runtime.shutdown().await;
    }

    #[tokio::test]
    #[ignore = "Requires API keys and network access"]
    async fn test_run_new_session_with_tool_usage() {
        dotenv().ok();
        let runtime = create_test_runtime().await;

        let mut config = create_test_session_config();
        config.tool_config = SessionToolConfig::read_only();
        config.tool_config.approval_policy = create_test_tool_approval_policy();
        let model = builtin::claude_sonnet_4_5();

        let result = OneShotRunner::run_new_session(
            &runtime.handle,
            config,
            "List the files in the current directory".to_string(),
            model,
        )
        .await
        .expect("New session run with tools should succeed with valid API key");

        assert!(!result.final_message.id().is_empty());
        println!(
            "New session run with tools succeeded: {:?}",
            result.final_message
        );

        let has_content = match &result.final_message.data {
            MessageData::Assistant { content, .. } => content.iter().any(|c| match c {
                AssistantContent::Text { text } => !text.is_empty(),
                _ => true,
            }),
            _ => false,
        };
        assert!(has_content, "Response should have some content");

        runtime.shutdown().await;
    }
}
