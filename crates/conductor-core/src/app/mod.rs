use crate::api::{Client as ApiClient, Model, ProviderKind, ToolCall};
use crate::app::command::ApprovalType;
use crate::app::conversation::{
    AppCommandType, AssistantContent, CompactResult, ToolResult, UserContent,
};
use crate::config::LlmConfigProvider;
use crate::error::{Error, Result};
use conductor_tools::ToolError;
use conductor_tools::tools::BASH_TOOL_NAME;
use conductor_tools::tools::bash::BashParams;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use tracing::{debug, error, info, warn};
use uuid;

pub mod adapters;
mod agent_executor;
pub mod cancellation;
pub mod command;
pub mod context;
pub mod context_util;
pub mod conversation;
pub mod io;

mod environment;

mod tool_executor;
pub mod validation;

#[cfg(test)]
mod tests;

use crate::app::context::TaskOutcome;

pub use cancellation::CancellationInfo;
pub use command::AppCommand;
pub use context::OpContext;
pub use conversation::{Conversation, Message};
pub use environment::EnvironmentInfo;
pub use tool_executor::ToolExecutor;

pub use agent_executor::{
    AgentEvent, AgentExecutor, AgentExecutorError, AgentExecutorRunRequest, ApprovalDecision,
};

#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        message: Message,
        model: Model,
    },
    MessageUpdated {
        id: String,
        content: String,
    },
    MessagePart {
        id: String,
        delta: String,
    },

    ToolCallStarted {
        name: String,
        id: String,
        model: Model,
    },
    ToolCallCompleted {
        name: String,
        result: conductor_tools::result::ToolResult,
        id: String,
        model: Model,
    },
    ToolCallFailed {
        name: String,
        error: String,
        id: String,
        model: Model,
    },
    ThinkingStarted,
    ThinkingCompleted,
    CommandResponse {
        command: conversation::AppCommandType,
        response: conversation::CommandResponse,
        id: String,
    },
    RequestToolApproval {
        name: String,
        parameters: serde_json::Value,
        id: String,
    },
    OperationCancelled {
        info: CancellationInfo,
    },
    ModelChanged {
        model: Model,
    },
    Error {
        message: String,
    },
    WorkspaceChanged,
    WorkspaceFiles {
        files: Vec<String>,
    },
}

#[derive(Clone)]
pub struct AppConfig {
    pub llm_config_provider: LlmConfigProvider,
}

impl Default for AppConfig {
    fn default() -> Self {
        // Create an in-memory auth storage for default
        let storage = Arc::new(crate::test_utils::InMemoryAuthStorage::new());
        let provider = LlmConfigProvider::new(storage);

        Self {
            llm_config_provider: provider,
        }
    }
}

pub struct App {
    pub config: AppConfig,
    pub conversation: Arc<Mutex<Conversation>>,
    pub tool_executor: Arc<ToolExecutor>,
    pub api_client: ApiClient,
    agent_executor: AgentExecutor,
    event_sender: mpsc::Sender<AppEvent>,
    approved_tools: Arc<tokio::sync::RwLock<HashSet<String>>>, // Tracks tools approved with "Always" for the session
    approved_bash_patterns: std::sync::Arc<tokio::sync::RwLock<HashSet<String>>>, // Tracks bash commands approved for the session
    current_op_context: Option<OpContext>,
    current_model: Model,
    session_config: Option<crate::session::state::SessionConfig>, // For tool visibility filtering
    workspace: Option<Arc<dyn crate::workspace::Workspace>>, // Workspace for environment and tool execution
    cached_system_prompt: Option<String>, // Cached system prompt to avoid recomputation
}

/// Check if a command matches any pattern in the given list
fn matches_any_pattern(command: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        // Check exact match first
        if pattern == command {
            return true;
        }
        // Then check glob pattern
        glob::Pattern::new(pattern)
            .map(|p| p.matches(command))
            .unwrap_or(false)
    })
}

impl App {
    pub async fn new_with_conversation(
        config: AppConfig,
        event_tx: mpsc::Sender<AppEvent>,
        initial_model: Model,
        workspace: Arc<dyn crate::workspace::Workspace>,
        tool_executor: Arc<ToolExecutor>,
        session_config: Option<crate::session::state::SessionConfig>,
        conversation: Conversation,
    ) -> Result<Self> {
        let api_client = ApiClient::new_with_provider(config.llm_config_provider.clone());
        let agent_executor = AgentExecutor::new(Arc::new(api_client.clone()));

        // Initialize approved_bash_patterns from session config
        let approved_bash_patterns = if let Some(ref sc) = session_config {
            if let Some(bash_config) = sc.tool_config.tools.get("bash") {
                let crate::session::state::ToolSpecificConfig::Bash(bash) = bash_config;
                bash.approved_patterns.iter().cloned().collect()
            } else {
                HashSet::new()
            }
        } else {
            HashSet::new()
        };
        let approved_bash_patterns =
            std::sync::Arc::new(tokio::sync::RwLock::new(approved_bash_patterns));

        Ok(Self {
            config,
            conversation: Arc::new(Mutex::new(conversation)),
            tool_executor,
            api_client,
            agent_executor,
            event_sender: event_tx,
            approved_tools: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            approved_bash_patterns,
            current_op_context: None,
            current_model: initial_model,
            session_config,
            workspace: Some(workspace),
            cached_system_prompt: None,
        })
    }

    pub async fn new(
        config: AppConfig,
        event_tx: mpsc::Sender<AppEvent>,
        initial_model: Model,
        workspace: Arc<dyn crate::workspace::Workspace>,
        tool_executor: Arc<ToolExecutor>,
        session_config: Option<crate::session::state::SessionConfig>,
    ) -> Result<Self> {
        let conversation = Conversation::new();
        Self::new_with_conversation(
            config,
            event_tx,
            initial_model,
            workspace,
            tool_executor,
            session_config,
            conversation,
        )
        .await
    }

    pub(crate) fn emit_event(&self, event: AppEvent) {
        match self.event_sender.try_send(event.clone()) {
            Ok(_) => {
                // Skip logging message parts for brevity
                if !matches!(event, AppEvent::MessagePart { .. }) {
                    debug!(target: "app.emit_event", "Sent event: {:?}", event);
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                warn!(target: "app.emit_event", "Event channel full, discarding event: {:?}", event);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!(target: "app.emit_event", "Event channel closed, discarding event: {:?}", event);
            }
        }
    }

    pub(crate) async fn emit_workspace_files(&self) {
        if let Some(workspace) = &self.workspace {
            info!(target: "app.emit_workspace_files", "Attempting to list workspace files...");
            match workspace.list_files(None, Some(10000)).await {
                Ok(files) => {
                    info!(target: "app.emit_workspace_files", "Emitting workspace files event with {} files", files.len());
                    if files.is_empty() {
                        warn!(target: "app.emit_workspace_files", "No files found in workspace - check if directory is correct");
                    }
                    self.emit_event(AppEvent::WorkspaceFiles { files });
                }
                Err(e) => {
                    warn!(target: "app.emit_workspace_files", "Failed to list workspace files: {}", e);
                }
            }
        } else {
            warn!(target: "app.emit_workspace_files", "No workspace available to list files");
        }
    }

    pub fn get_current_model(&self) -> Model {
        self.current_model
    }

    /// Gets or creates the system prompt, using cache if available
    async fn get_or_create_system_prompt(&mut self) -> Result<String> {
        if let Some(ref cached) = self.cached_system_prompt {
            debug!(target: "app.get_or_create_system_prompt", "Using cached system prompt");
            Ok(cached.clone())
        } else {
            debug!(target: "app.get_or_create_system_prompt", "Creating new system prompt");
            let prompt = if let Some(workspace) = &self.workspace {
                create_system_prompt_with_workspace(Some(self.current_model), workspace.as_ref())
                    .await?
            } else {
                create_system_prompt(Some(self.current_model))?
            };
            // Cache the system prompt
            self.cached_system_prompt = Some(prompt.clone());
            Ok(prompt)
        }
    }

    pub async fn set_model(&mut self, model: Model) -> Result<()> {
        // Check if the provider is available (has API key or OAuth)
        let provider = model.provider();
        let auth = self
            .config
            .llm_config_provider
            .get_auth_for_provider(provider)
            .await?;
        if auth.is_none() {
            return Err(crate::error::Error::Configuration(format!(
                "Cannot set model to {}: missing authentication for {} provider",
                model.as_ref(),
                match provider {
                    ProviderKind::Anthropic => "Anthropic",
                    ProviderKind::OpenAI => "OpenAI",
                    ProviderKind::Google => "Google",
                    ProviderKind::XAI => "xAI",
                }
            )));
        }

        // Set the model
        self.current_model = model;

        // Clear cached system prompt when model changes
        self.cached_system_prompt = None;

        // Emit an event to notify UI of the change
        self.emit_event(AppEvent::ModelChanged { model });

        Ok(())
    }

    pub async fn add_message(&self, message: Message) {
        let mut conversation_guard = self.conversation.lock().await;
        conversation_guard.messages.push(message.clone());
        drop(conversation_guard);

        // Emit event only for non-tool messages
        if !matches!(message, Message::Tool { .. }) {
            self.emit_event(AppEvent::MessageAdded {
                message,
                model: self.current_model,
            });
        }
    }

    // Renamed from process_user_message to make it clear it starts an op
    // Returns the event receiver if a standard agent operation was started
    pub async fn process_user_message(
        &mut self,
        message: String,
    ) -> Result<Option<mpsc::Receiver<AgentEvent>>> {
        // Cancel any existing operations first
        self.cancel_current_processing().await;

        // Check for incomplete tool calls and inject cancelled tool results
        self.inject_cancelled_tool_results().await;

        // Create a new operation context
        let op_context = OpContext::new();
        self.current_op_context = Some(op_context);

        // Add user message
        let (thread_id, parent_id) = {
            let conv = self.conversation.lock().await;
            (
                conv.current_thread_id,
                conv.messages.last().map(|m| m.id().to_string()),
            )
        };

        self.add_message(Message::User {
            content: vec![UserContent::Text {
                text: message.clone(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id,
            parent_message_id: parent_id,
        })
        .await;

        // Start thinking and spawn agent operation
        self.emit_event(AppEvent::ThinkingStarted);
        match self.spawn_agent_operation().await {
            Ok(maybe_receiver) => Ok(maybe_receiver), // Return the receiver
            Err(e) => {
                error!(target:
                    "App.start_standard_operation",
                    "Error spawning agent operation task: {}", e,
                );
                self.emit_event(AppEvent::ThinkingCompleted); // Stop thinking on spawn error
                self.emit_event(AppEvent::Error {
                    message: format!("Failed to start agent operation: {e}"),
                });
                self.current_op_context = None; // Clean up context
                Err(e)
            }
        }
    }

    async fn spawn_agent_operation(&mut self) -> Result<Option<mpsc::Receiver<AgentEvent>>> {
        debug!(target:
            "app.spawn_agent_operation",
            "Spawning agent operation task...",
        );

        // Get tools for the operation
        // Always get all tools from the tool executor (includes workspace tools)
        let mut tool_schemas = self.tool_executor.get_tool_schemas().await;

        // Then apply visibility filtering if we have a session config
        if let Some(session_config) = &self.session_config {
            tool_schemas = session_config.filter_tools_by_visibility(tool_schemas);
        }

        // Get or create system prompt from cache (before accessing op_context mutably)
        let system_prompt = self.get_or_create_system_prompt().await?;

        // Get mutable access to OpContext and its token
        let op_context = match &mut self.current_op_context {
            Some(ctx) => ctx,
            None => {
                return Err(crate::error::Error::InvalidOperation(
                    "No operation context available to spawn agent operation".to_string(),
                ));
            }
        };
        let token = op_context.cancel_token.clone();

        // Get messages (snapshot)
        let api_messages = {
            let conversation_guard = self.conversation.lock().await;
            conversation_guard
                .get_thread_messages()
                .into_iter()
                .cloned()
                .collect()
        };

        let current_model = self.current_model;
        let agent_executor = self.agent_executor.clone();

        // --- Tool Approval Callback ---
        let approved_tools_for_approval = self.approved_tools.clone();
        let tool_executor_for_approval = self.tool_executor.clone();
        let command_tx_for_approval = OpContext::command_tx().clone();
        let approved_bash_patterns_clone = self.approved_bash_patterns.clone(); // Clone for capture
        let session_config_clone = self.session_config.clone(); // Clone for capture

        let tool_approval_callback = move |tool_call: ToolCall| {
            let approved_tools = approved_tools_for_approval.clone();
            let executor = tool_executor_for_approval.clone();
            let command_tx = command_tx_for_approval.clone();
            let tool_name = tool_call.name.clone();
            let tool_id = tool_call.id.clone();
            let approved_bash_patterns = approved_bash_patterns_clone.clone();
            let session_config = session_config_clone.clone();

            async move {
                match executor.requires_approval(&tool_name).await {
                    Ok(false) => return Ok(ApprovalDecision::Approved),
                    Ok(true) => {}
                    Err(e) => {
                        return Err(ToolError::InternalError(format!(
                            "Failed to check tool approval status for {tool_name}: {e}"
                        )));
                    }
                };

                if approved_tools.read().await.contains(&tool_name) {
                    return Ok(ApprovalDecision::Approved);
                }

                // Check if this is a bash command that matches an approved pattern
                if tool_name == BASH_TOOL_NAME {
                    // Extract command from tool parameters
                    let params: BashParams = match serde_json::from_value(
                        tool_call.parameters.clone(),
                    ) {
                        Ok(p) => p,
                        Err(e) => {
                            debug!(tool_id=%tool_id, tool_name=%tool_name, "Failed to parse BashParams from tool_call.parameters: {}", e);
                            return Err(ToolError::invalid_params(
                                "bash",
                                format!("Failed to parse BashParams: {e}"),
                            ));
                        }
                    };

                    // Check against static patterns from session config
                    let static_patterns = if let Some(ref session_config) = session_config {
                        if let Some(bash_config) = session_config.tool_config.tools.get("bash") {
                            let crate::session::state::ToolSpecificConfig::Bash(bash) = bash_config;
                            &bash.approved_patterns
                        } else {
                            &Vec::new()
                        }
                    } else {
                        &Vec::new()
                    };

                    if matches_any_pattern(params.command.as_str(), static_patterns) {
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Bash command {} matches static patterns: {:?}", params.command, static_patterns);
                        return Ok(ApprovalDecision::Approved);
                    } else {
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Bash command {} does not match static patterns: {:?}", params.command, static_patterns);
                    }

                    // Check against dynamically approved patterns (convert HashSet to Vec)
                    let dynamic_patterns: Vec<String> = {
                        let patterns = approved_bash_patterns.read().await;
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Dynamic patterns: {:?}", patterns);
                        patterns.iter().cloned().collect()
                    };

                    if matches_any_pattern(params.command.as_str(), &dynamic_patterns) {
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Bash command {} matches dynamic patterns: {:?}", params.command, dynamic_patterns);
                        return Ok(ApprovalDecision::Approved);
                    } else {
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Bash command {} does not match dynamic patterns: {:?}", params.command, dynamic_patterns);
                    }
                }

                // Needs interactive approval - create oneshot channel for receiving the decision
                // Needs interactive approval - create oneshot channel for receiving the decision
                let (tx, rx) = oneshot::channel();

                // Send approval request to the actor loop via command channel
                if let Err(e) = command_tx
                    .send(AppCommand::RequestToolApprovalInternal {
                        tool_call,
                        responder: tx,
                    })
                    .await
                {
                    // If we can't send the request, treat as an error
                    error!(tool_id=%tool_id, tool_name=%tool_name, "Failed to send tool approval request: {}", e);
                    return Err(ToolError::InternalError(format!(
                        "Failed to request tool approval: {e}"
                    )));
                }

                // Wait for the decision
                match rx.await {
                    Ok(d) => Ok(d), // User made a choice
                    Err(_) => {
                        // Responder was dropped (likely due to cancellation elsewhere or shutdown)
                        warn!(tool_id=%tool_id, tool_name=%tool_name, "Approval decision channel closed for tool.");
                        Ok(ApprovalDecision::Denied) // Treat as denied
                    }
                }
            }
        };

        // --- Tool Execution Callback ---
        let tool_executor_for_execution = self.tool_executor.clone();
        let tool_execution_callback =
            move |tool_call: ToolCall, callback_token: CancellationToken| {
                let executor = tool_executor_for_execution.clone();
                let tool_name = tool_call.name.clone();
                let tool_id = tool_call.id.clone();

                async move {
                    info!(tool_id=%tool_id, tool_name=%tool_name, "Executing tool via callback.");
                    executor
                        .execute_tool_with_cancellation(&tool_call, callback_token)
                        .await
                }
            };

        let (agent_event_tx, agent_event_rx) = mpsc::channel(100);
        op_context.tasks.spawn(async move {
            debug!(target:
                "spawn_agent_operation task",
                "Agent operation task started.",
            );
            let request = AgentExecutorRunRequest {
                model: current_model,
                initial_messages: api_messages,
                system_prompt: Some(system_prompt),
                available_tools: tool_schemas,
                tool_approval_callback,
                tool_execution_callback,
            };
            let operation_result = agent_executor
                .run(request, agent_event_tx, token)
                .await;

            debug!(target: "spawn_agent_operation task", "Agent operation task finished with result: {:?}", operation_result.is_ok());

            TaskOutcome::AgentOperationComplete {
                result: operation_result,
            }
        });

        debug!(target:
            "app.spawn_agent_operation",
            "Agent operation task successfully spawned.",
        );
        Ok(Some(agent_event_rx))
    }

    // Modified handle_command to return only the response string (or None)
    // It now starts tasks directly but doesn't return the receiver.
    pub async fn handle_command(
        &mut self,
        command: AppCommandType,
    ) -> Result<Option<conversation::CommandResponse>> {
        // Cancel any previous operation before starting a command
        // Note: This is also called by start_standard_operation if user input isn't a command
        self.cancel_current_processing().await;

        match command {
            AppCommandType::Clear => {
                self.conversation.lock().await.clear();
                self.approved_tools.write().await.clear(); // Also clear tool approvals
                self.cached_system_prompt = None; // Clear cached system prompt
                Ok(Some(conversation::CommandResponse::Text(
                    "Conversation and tool approvals cleared.".to_string(),
                )))
            }
            AppCommandType::Compact => {
                // Create OpContext for cancellable command
                let op_context = OpContext::new();
                self.current_op_context = Some(op_context);
                let token = self
                    .current_op_context
                    .as_ref()
                    .unwrap()
                    .cancel_token
                    .clone();

                // Spawn the compaction task within the context
                // TODO: Add TaskOutcome::CompactResult and handle in actor loop
                // For now, await directly and clear context
                warn!(target:
                    "handle_command",
                    "Compact command task spawning needs TaskOutcome handling in actor loop.",
                );
                let result = match self.compact_conversation(token).await {
                    Ok(result) => Ok(Some(conversation::CommandResponse::Compact(result))),
                    Err(e) => {
                        error!(target:
                            "App.handle_command",
                            "Error during compact: {}", e,
                        );
                        Err(e) // Propagate actual errors
                    }
                }?;
                self.current_op_context = None; // Clear context after command
                Ok(result)
            }
            AppCommandType::Help => {
                Ok(Some(conversation::CommandResponse::Text(build_help_text())))
            }
            AppCommandType::Auth => Ok(Some(conversation::CommandResponse::Text(
                "Authentication configuration is available through the TUI.\n\
                    Exit this session and run 'conductor auth setup' to configure authentication."
                    .to_string(),
            ))),
            AppCommandType::Model { target } => {
                if target.is_none() {
                    // If no model specified, list available models
                    use crate::api::Model;
                    use strum::IntoEnumIterator;

                    let current_model = self.get_current_model();
                    let available_models: Vec<String> = Model::iter()
                        .map(|m| {
                            let model_str = m.as_ref();
                            let aliases = m.aliases();
                            let alias_str = if aliases.is_empty() {
                                String::new()
                            } else if aliases.len() == 1 {
                                format!(" (alias: {})", aliases[0])
                            } else {
                                format!(" (aliases: {})", aliases.join(", "))
                            };

                            if m == current_model {
                                format!("* {model_str}{alias_str}") // Mark current model with asterisk
                            } else {
                                format!("  {model_str}{alias_str}")
                            }
                        })
                        .collect();

                    Ok(Some(conversation::CommandResponse::Text(format!(
                        "Current model: {}\nAvailable models:\n{}",
                        current_model.as_ref(),
                        available_models.join("\n")
                    ))))
                } else if let Some(ref model_name) = target {
                    // Try to set the model
                    use crate::api::Model;
                    use std::str::FromStr;

                    match Model::from_str(model_name) {
                        Ok(model) => match self.set_model(model).await {
                            Ok(()) => Ok(Some(conversation::CommandResponse::Text(format!(
                                "Model changed to {}",
                                model.as_ref()
                            )))),
                            Err(e) => Ok(Some(conversation::CommandResponse::Text(format!(
                                "Failed to set model: {e}"
                            )))),
                        },
                        Err(_) => Ok(Some(conversation::CommandResponse::Text(format!(
                            "Unknown model: {model_name}"
                        )))),
                    }
                } else {
                    // This should not happen with the current enum structure
                    Ok(None)
                }
            }
            AppCommandType::Cancel => {
                // The cancel command is handled differently - it needs to be processed
                // by the TUI or other client to actually cancel operations
                Ok(Some(conversation::CommandResponse::Text(
                    "Use the cancel shortcut or UI element to cancel operations.".to_string(),
                )))
            }
        }
    }

    pub async fn compact_conversation(
        &mut self,
        token: CancellationToken,
    ) -> Result<CompactResult> {
        info!(target:"App.compact_conversation", "Compacting conversation...");
        let client = self.api_client.clone();
        let conversation_arc = self.conversation.clone();
        let model = self.current_model;

        // Run directly but make it cancellable.
        let result = tokio::select! {
            biased;
            res = async { conversation_arc.lock().await.compact(&client, model, token.clone()).await } => res.map_err(|e| Error::InvalidOperation(format!("Compact failed: {e}")))?,
            _ = token.cancelled() => {
                 info!(target:"App.compact_conversation", "Compaction cancelled.");
                 return Ok(CompactResult::Cancelled);
             }
        };

        info!(target:"App.compact_conversation", "Conversation compacted.");
        Ok(result)
    }

    pub async fn cancel_current_processing(&mut self) {
        // Use operation context for cancellation if available
        if let Some(mut op_context) = self.current_op_context.take() {
            info!(target:
                "App.cancel_current_processing",
                "Cancelling current operation via OpContext",
            );

            // Capture the current state for the cancellation info
            let active_tools = op_context.active_tools.values().cloned().collect();
            // TODO: Get accurate pending approval status from the actor loop's ApprovalState
            let cancellation_info = CancellationInfo {
                api_call_in_progress: false, // Handled by AgentExecutor now
                active_tools,
                pending_tool_approvals: false, // TODO: Update this based on actor state
            };

            op_context.cancel_and_shutdown().await;

            self.emit_event(AppEvent::OperationCancelled {
                info: cancellation_info,
            });
            // Don't return here, actor loop needs to clear receiver if present
        } else {
            warn!(target:
                "App.cancel_current_processing",
                "Attempted to cancel processing, but no active operation context was found.",
            );
        }
        // Clearing the receiver is now handled in handle_app_command
    }

    /// Inject cancelled tool results for any incomplete tool calls in the conversation.
    /// This ensures that the Anthropic API receives proper tool_result blocks for every tool_use block.
    pub async fn inject_cancelled_tool_results(&mut self) {
        let incomplete_tool_calls = {
            let conversation_guard = self.conversation.lock().await;
            self.find_incomplete_tool_calls(&conversation_guard)
        };

        if !incomplete_tool_calls.is_empty() {
            info!(target: "App.inject_cancelled_tool_results",
                  "Found {} incomplete tool calls, injecting cancellation results",
                  incomplete_tool_calls.len());

            for tool_call in incomplete_tool_calls {
                let (thread_id, parent_id) = {
                    let conv = self.conversation.lock().await;
                    (
                        conv.current_thread_id,
                        conv.messages.last().map(|m| m.id().to_string()),
                    )
                };

                let cancelled_result = Message::Tool {
                    tool_use_id: tool_call.id.clone(),
                    result: ToolResult::Error(ToolError::Cancelled(tool_call.name.clone())),
                    timestamp: Message::current_timestamp(),
                    id: Message::generate_id("tool", Message::current_timestamp()),
                    thread_id,
                    parent_message_id: parent_id,
                };

                // Add the message using the standard add_message method to ensure proper event emission
                self.add_message(cancelled_result).await;
                debug!(target: "App.inject_cancelled_tool_results",
                       "Injected cancellation result for tool call: {} ({})",
                       tool_call.name, tool_call.id);
            }
        }
    }

    /// Find tool calls that don't have corresponding tool results
    fn find_incomplete_tool_calls(&self, conversation: &Conversation) -> Vec<ToolCall> {
        let mut tool_calls = Vec::new();
        let mut tool_results = std::collections::HashSet::new();

        // First pass: collect all tool results
        for message in &conversation.messages {
            if let Message::Tool { tool_use_id, .. } = message {
                tool_results.insert(tool_use_id.clone());
            }
        }

        // Second pass: find tool calls without results
        for message in &conversation.messages {
            if let Message::Assistant { content, .. } = message {
                for block in content {
                    if let AssistantContent::ToolCall { tool_call } = block {
                        if !tool_results.contains(&tool_call.id) {
                            tool_calls.push(tool_call.clone());
                        }
                    }
                }
            }
        }

        tool_calls
    }

    // Helper methods for testing
    #[cfg(test)]
    pub async fn add_user_message(&mut self, content: &str) -> Result<()> {
        use crate::app::conversation::UserContent;
        let conversation = self.conversation.clone();
        let mut conversation_guard = conversation.lock().await;
        let parent_id = conversation_guard
            .messages
            .last()
            .map(|m| m.id().to_string());
        let message = Message::User {
            content: vec![UserContent::Text {
                text: content.to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("user", Message::current_timestamp()),
            thread_id: conversation_guard.current_thread_id,
            parent_message_id: parent_id,
        };
        conversation_guard.add_message(message);
        Ok(())
    }

    #[cfg(test)]
    pub async fn add_assistant_message(&mut self, content: &str) -> Result<()> {
        use crate::app::conversation::AssistantContent;
        let conversation = self.conversation.clone();
        let mut conversation_guard = conversation.lock().await;
        let parent_id = conversation_guard
            .messages
            .last()
            .map(|m| m.id().to_string());
        let message = Message::Assistant {
            content: vec![AssistantContent::Text {
                text: content.to_string(),
            }],
            timestamp: Message::current_timestamp(),
            id: Message::generate_id("assistant", Message::current_timestamp()),
            thread_id: conversation_guard.current_thread_id,
            parent_message_id: parent_id,
        };
        conversation_guard.add_message(message);
        Ok(())
    }

    #[cfg(test)]
    pub async fn get_messages(&self) -> Vec<Message> {
        let conversation_guard = self.conversation.lock().await;
        conversation_guard
            .get_thread_messages()
            .into_iter()
            .cloned()
            .collect()
    }

    #[cfg(test)]
    pub async fn edit_message(
        &mut self,
        message_id: &str,
        new_content: &str,
    ) -> Result<Option<uuid::Uuid>> {
        use crate::app::conversation::UserContent;
        let conversation = self.conversation.clone();
        let mut conversation_guard = conversation.lock().await;
        let new_thread_id = conversation_guard.edit_message(
            message_id,
            vec![UserContent::Text {
                text: new_content.to_string(),
            }],
        );
        Ok(new_thread_id)
    }
}

// Approval queue helper struct
struct ApprovalQueue {
    current: Option<(String, ToolCall, oneshot::Sender<ApprovalDecision>)>,
    queued: std::collections::VecDeque<(String, ToolCall, oneshot::Sender<ApprovalDecision>)>,
}

impl ApprovalQueue {
    fn new() -> Self {
        Self {
            current: None,
            queued: std::collections::VecDeque::new(),
        }
    }

    fn add_request(
        &mut self,
        id: String,
        tool_call: ToolCall,
        responder: oneshot::Sender<ApprovalDecision>,
    ) {
        self.queued.push_back((id, tool_call, responder));
    }

    fn cancel_all(&mut self) {
        // Drop current request responder to signal cancellation
        if let Some((id, _, _)) = self.current.take() {
            info!(target: "ApprovalQueue", "Cancelled active tool approval request for ID '{}'", id);
        }
        // Drop all queued responders
        if !self.queued.is_empty() {
            info!(target: "ApprovalQueue", "Clearing {} queued tool approval requests", self.queued.len());
            self.queued.clear();
        }
    }
}

// Define the App actor loop function with minimal refactoring
pub async fn app_actor_loop(mut app: App, mut command_rx: mpsc::Receiver<AppCommand>) {
    info!(target: "app_actor_loop", "App actor loop started.");

    // Emit initial workspace files for the UI
    app.emit_workspace_files().await;

    // Approval queue state
    let mut approval_queue = ApprovalQueue::new();

    // Active agent event receiver
    let mut active_agent_event_rx: Option<mpsc::Receiver<AgentEvent>> = None;

    // Track if the associated task for the active receiver has completed
    let mut agent_task_completed = false;

    loop {
        tokio::select! {
            // Handle incoming commands from the UI/Main thread
            Some(command) = command_rx.recv() => {
                match handle_app_command(
                    &mut app,
                    command,
                    &mut approval_queue,
                    &mut active_agent_event_rx,
                )
                .await
                {
                    Ok(should_exit) => {
                        if should_exit {
                            break; // Exit loop if Shutdown command was received
                        }
                    }
                    Err(e) => {
                        error!(target: "app_actor_loop", "Error handling app command: {}", e);
                        app.emit_event(AppEvent::Error {
                            message: format!("Command failed: {e}"),
                        });
                    }
                }
                // Reset task completion flag only if a new operation started
                if active_agent_event_rx.is_some() {
                    debug!(target: "app_actor_loop", "Resetting agent_task_completed flag due to new operation.");
                    agent_task_completed = false;
                }
            }

            // Poll for completed tasks (Agent Operations) from OpContext
            // This arm MUST be polled *before* the event receiver arm
            result = async {
                if let Some(ctx) = app.current_op_context.as_mut() {
                    if ctx.tasks.is_empty() { None } else { ctx.tasks.join_next().await }
                } else {
                    None
                }
            }, if app.current_op_context.is_some() => {
                if let Some(join_result) = result {
                    match join_result {
                        Ok(task_outcome) => {
                            let is_standard_op = matches!(task_outcome, TaskOutcome::AgentOperationComplete{..});

                            handle_task_outcome(&mut app, task_outcome).await;

                            if is_standard_op {
                                debug!(target: "app_actor_loop", "Agent task completed flag set to true.");
                                agent_task_completed = true;
                            }

                            // Check if we should signal ThinkingCompleted now
                            if agent_task_completed && active_agent_event_rx.is_none() {
                                debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (Task done, receiver drained).");
                                app.emit_event(AppEvent::ThinkingCompleted);
                                agent_task_completed = false;
                            }
                        }
                        Err(join_err) => {
                            error!(target: "app_actor_loop", "Task join error: {}", join_err);
                            app.current_op_context = None;
                            active_agent_event_rx = None;
                            agent_task_completed = false;
                            app.emit_event(AppEvent::ThinkingCompleted);
                            app.emit_event(AppEvent::Error {
                                message: format!("A task failed unexpectedly: {join_err}")
                            });
                        }
                    }
                } else {
                    // JoinSet returned None - all tasks finished
                    if let Some(_ctx) = app.current_op_context.take() {
                        debug!(target: "app_actor_loop", "JoinSet empty. Clearing context.");
                        agent_task_completed = true;

                        if agent_task_completed && active_agent_event_rx.is_none() {
                            debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (JoinSet empty, receiver drained).");
                            app.emit_event(AppEvent::ThinkingCompleted);
                            agent_task_completed = false;
                        }
                    }
                }
            }

            // Poll for incoming AgentEvents
            // Poll this *after* task completion to ensure correct ordering
            maybe_agent_event = async { active_agent_event_rx.as_mut().unwrap().recv().await },
            if active_agent_event_rx.is_some() => {
                match maybe_agent_event {
                    Some(event) => {
                        handle_agent_event(&mut app, event).await;
                    }
                    None => {
                        // Channel closed
                        debug!(target: "app_actor_loop", "Agent event channel closed.");
                        active_agent_event_rx = None;

                        if agent_task_completed {
                            debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (Receiver closed, task completed).");
                            app.emit_event(AppEvent::ThinkingCompleted);
                            agent_task_completed = false;
                        }
                    }
                }
            }

            // Default branch if no other arms are ready
            else => {}
        }
    }
    info!(target: "app_actor_loop", "App actor loop finished.");
}

// Process the next approval request from the queue
async fn process_next_approval_request(app: &mut App, queue: &mut ApprovalQueue) {
    if queue.current.is_some() {
        debug!(target: "process_next_approval", "An approval request is already active.");
        return;
    }

    while let Some((id, tool_call, responder)) = queue.queued.pop_front() {
        if app.approved_tools.read().await.contains(&tool_call.name) {
            info!(target: "process_next_approval", "Auto-approving tool '{}' (ID: {})", tool_call.name, id);
            if responder.send(ApprovalDecision::Approved).is_err() {
                warn!(target: "process_next_approval", "Failed to send auto-approval for tool ID '{}'", id);
            }
        } else {
            // Not auto-approved, send to UI
            info!(target: "process_next_approval", "Sending tool approval request to UI for '{}' (ID: {})", tool_call.name, id);
            let parameters = tool_call.parameters.clone();
            let name = tool_call.name.clone();

            queue.current = Some((id.clone(), tool_call, responder));

            app.emit_event(AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
            });
            return;
        }
    }
    debug!(target: "process_next_approval", "Approval queue processed.");
}

// Handle app commands
async fn handle_app_command(
    app: &mut App,
    command: AppCommand,
    approval_queue: &mut ApprovalQueue,
    active_agent_event_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
) -> Result<bool> {
    debug!(target: "handle_app_command", "Received command: {:?}", command);

    match command {
        AppCommand::ProcessUserInput(message) => {
            if message.starts_with('/') {
                // Clear previous receiver if any before running command
                if active_agent_event_rx.is_some() {
                    warn!(target: "handle_app_command", "Clearing previous active agent event receiver due to new command input.");
                    *active_agent_event_rx = None;
                }
                match AppCommandType::parse(&message) {
                    Ok(cmd) => {
                        handle_slash_command(app, cmd).await;
                    }
                    Err(e) => {
                        app.emit_event(AppEvent::Error {
                            message: format!("Error parsing command: {e:?}"),
                        });
                    }
                }
            } else {
                // Regular user message, start a standard operation
                if active_agent_event_rx.is_some() {
                    warn!(target: "handle_app_command", "Clearing previous active agent event receiver due to new user input.");
                    *active_agent_event_rx = None;
                }
                match app.process_user_message(message).await {
                    Ok(maybe_receiver) => {
                        *active_agent_event_rx = maybe_receiver;
                    }
                    Err(e) => {
                        error!(target: "handle_app_command", "Error starting standard operation: {}", e);
                    }
                }
            }
            Ok(false)
        }
        AppCommand::EditMessage {
            message_id,
            new_content,
        } => {
            debug!(target: "handle_app_command", "Editing message {} with new content", message_id);

            // Cancel any existing operations first
            app.cancel_current_processing().await;
            if active_agent_event_rx.is_some() {
                warn!(target: "handle_app_command", "Clearing previous active agent event receiver due to message edit.");
                *active_agent_event_rx = None;
            }

            // Edit the message in the conversation. This removes the old branch and adds the new one.
            let (new_thread_id, edited_message_opt) = {
                let mut conversation = app.conversation.lock().await;
                let new_thread_id = conversation.edit_message(
                    &message_id,
                    vec![UserContent::Text {
                        text: new_content.clone(),
                    }],
                );

                // Attempt to fetch the newly created edited message (it will be the latest User message in the new thread)
                let edited_msg = new_thread_id.and_then(|tid| {
                    conversation
                        .messages
                        .iter()
                        .rev()
                        .find(|m| m.thread_id() == &tid && matches!(m, Message::User { .. }))
                        .cloned()
                });

                (new_thread_id, edited_msg)
            };

            // Notify the UI about the newly edited user message so it can appear immediately
            if let Some(edited_message) = edited_message_opt {
                app.emit_event(AppEvent::MessageAdded {
                    message: edited_message,
                    model: app.current_model,
                });
            }

            if let Some(thread_id) = new_thread_id {
                debug!(target: "handle_app_command", "Created new branch with thread_id: {}", thread_id);

                // This message is now the latest in the conversation.
                // We can now start a new agent operation.
                // This logic is adapted from `process_user_message`, but without adding a new message.
                app.inject_cancelled_tool_results().await;
                app.current_op_context = Some(OpContext::new());
                app.emit_event(AppEvent::ThinkingStarted);

                match app.spawn_agent_operation().await {
                    Ok(maybe_receiver) => {
                        *active_agent_event_rx = maybe_receiver;
                    }
                    Err(e) => {
                        error!(target: "handle_app_command", "Error processing edited message: {}", e);
                        app.current_op_context = None;
                        app.emit_event(AppEvent::ThinkingCompleted);
                    }
                }
            } else {
                error!(target: "handle_app_command", "Failed to edit message {}", message_id);
            }
            Ok(false)
        }
        AppCommand::RestoreConversation {
            messages,
            approved_tools,
            approved_bash_patterns,
        } => {
            // Atomically restore entire conversation state
            debug!(target:"handle_app_command", "Restoring conversation with {} messages, {} approved tools, and {} approved bash patterns",
                messages.len(), approved_tools.len(), approved_bash_patterns.len());

            // Restore messages
            let mut conversation_guard = app.conversation.lock().await;
            conversation_guard.messages = messages;
            drop(conversation_guard);

            // Restore approved tools
            *app.approved_tools.write().await = approved_tools;

            // Restore approved bash patterns
            *app.approved_bash_patterns.write().await = approved_bash_patterns;

            debug!(target:"handle_app_command", "Conversation restoration complete");
            Ok(false)
        }

        AppCommand::HandleToolResponse { id, approval } => {
            if let Some((current_id, current_tool_call, responder)) = approval_queue.current.take()
            {
                debug!(target: "handle_app_command", "Handling tool response for ID '{}', approval: {:?}", id, approval);
                if current_id != id {
                    error!(target: "handle_app_command", "Mismatched tool ID. Expected '{}', got '{}'", current_id, id);
                    approval_queue
                        .queued
                        .push_front((current_id, current_tool_call, responder));
                } else {
                    let decision = match approval {
                        ApprovalType::Once => {
                            debug!(target: "handle_app_command", "Approving tool call with ID '{}' once.", id);
                            ApprovalDecision::Approved
                        }
                        ApprovalType::AlwaysTool => {
                            debug!(target: "handle_app_command", "Approving tool call with ID '{}' always.", id);
                            app.approved_tools
                                .write()
                                .await
                                .insert(current_tool_call.name.clone());
                            ApprovalDecision::Approved
                        }
                        ApprovalType::AlwaysBashPattern(pattern) => {
                            debug!(target: "handle_app_command", "Approving bash command '{}' always.", pattern);
                            app.approved_bash_patterns
                                .write()
                                .await
                                .insert(pattern.clone());
                            ApprovalDecision::Approved
                        }
                        ApprovalType::Denied => {
                            debug!(target: "handle_app_command", "Denying tool call with ID '{}'.", id);
                            ApprovalDecision::Denied
                        }
                    };

                    debug!(target: "handle_app_command", "Sending approval decision for tool ID '{}': {:?}", id, decision);
                    if responder.send(decision).is_err() {
                        error!(target: "handle_app_command", "Failed to send approval decision for tool ID '{}'", id);
                    }
                }
            } else {
                error!(target: "handle_app_command", "Received tool response for ID '{}' but no current approval request was active.", id);
            }

            process_next_approval_request(app, approval_queue).await;
            Ok(false)
        }

        AppCommand::CancelProcessing => {
            debug!(target: "handle_app_command", "Handling CancelProcessing command.");
            app.cancel_current_processing().await;
            approval_queue.cancel_all();

            if active_agent_event_rx.is_some() {
                debug!(target: "handle_app_command", "Clearing active agent event receiver due to cancellation.");
                *active_agent_event_rx = None;
            }
            app.emit_event(AppEvent::ThinkingCompleted);
            Ok(false)
        }

        AppCommand::ExecuteBashCommand { command } => {
            debug!(target: "handle_app_command", "Executing bash command: {}", command);

            // Create a tool call for the bash command
            let tool_call = ToolCall {
                id: format!("bash_{}", uuid::Uuid::new_v4()),
                name: "bash".to_string(),
                parameters: serde_json::json!({
                    "command": command,
                }),
            };

            // Create a cancellation token for the execution
            let token = CancellationToken::new();

            // Execute the bash tool directly (bypassing validation)
            match app
                .tool_executor
                .execute_tool_direct(&tool_call, token)
                .await
            {
                Ok(output) => {
                    // Get the formatted output from the typed result
                    let output_str = output.llm_format();

                    // Parse the output to extract stdout/stderr/exit code
                    // The bash tool returns output in a specific format
                    let (stdout, stderr, exit_code) = parse_bash_output(&output_str);

                    // Add the command execution as a message
                    let (thread_id, parent_id) = {
                        let conv = app.conversation.lock().await;
                        (
                            conv.current_thread_id,
                            conv.messages.last().map(|m| m.id().to_string()),
                        )
                    };

                    let message = Message::User {
                        content: vec![UserContent::CommandExecution {
                            command,
                            stdout,
                            stderr,
                            exit_code,
                        }],
                        timestamp: Message::current_timestamp(),
                        id: Message::generate_id("user", Message::current_timestamp()),
                        thread_id,
                        parent_message_id: parent_id,
                    };
                    app.add_message(message).await;

                    // A bash command can mutate the workspace; notify listeners.
                    app.emit_event(AppEvent::WorkspaceChanged);
                    app.emit_workspace_files().await;
                }
                Err(e) => {
                    error!(target: "handle_app_command", "Failed to execute bash command: {}", e);

                    // Add error as a command execution with error output
                    let (thread_id, parent_id) = {
                        let conv = app.conversation.lock().await;
                        (
                            conv.current_thread_id,
                            conv.messages.last().map(|m| m.id().to_string()),
                        )
                    };

                    let message = Message::User {
                        content: vec![UserContent::CommandExecution {
                            command,
                            stdout: String::new(),
                            stderr: format!("Error executing command: {e}"),
                            exit_code: -1,
                        }],
                        timestamp: Message::current_timestamp(),
                        id: Message::generate_id("user", Message::current_timestamp()),
                        thread_id,
                        parent_message_id: parent_id,
                    };
                    app.add_message(message).await;

                    // A bash command can mutate the workspace; notify listeners.
                    app.emit_event(AppEvent::WorkspaceChanged);
                    app.emit_workspace_files().await;
                }
            }
            Ok(false)
        }
        AppCommand::Shutdown => {
            info!(target: "handle_app_command", "Received Shutdown command.");
            app.cancel_current_processing().await;
            approval_queue.cancel_all();
            *active_agent_event_rx = None;
            Ok(true)
        }
        AppCommand::GetCurrentConversation => {
            // This command is now handled synchronously via RPC
            // Should not be called directly anymore
            warn!(target:"handle_app_command", "GetCurrentConversation command received - this should use the sync RPC instead");
            Ok(false)
        }
        AppCommand::RequestToolApprovalInternal {
            tool_call,
            responder,
        } => {
            let tool_id = tool_call.id.clone();
            let tool_name = tool_call.name.clone();

            info!(target: "handle_app_command", "Received internal request for tool approval: '{}' (ID: {})", tool_name, tool_id);

            approval_queue.add_request(tool_id, tool_call, responder);
            process_next_approval_request(app, approval_queue).await;
            Ok(false)
        }

        AppCommand::ExecuteCommand(cmd) => {
            warn!(target: "handle_app_command", "Received ExecuteCommand: {}", cmd.as_command_str());
            if active_agent_event_rx.is_some() {
                warn!(target: "handle_app_command", "Clearing previous active agent event receiver due to ExecuteCommand.");
                *active_agent_event_rx = None;
            }
            handle_slash_command(app, cmd).await;
            Ok(false)
        }

        AppCommand::RequestWorkspaceFiles => {
            info!(target: "app.handle_app_command", "Received RequestWorkspaceFiles command");
            app.emit_workspace_files().await;
            Ok(false)
        }
    }
}

// Handle slash commands
async fn handle_slash_command(app: &mut App, command: AppCommandType) {
    let command_str = command.as_command_str();
    match app.handle_command(command.clone()).await {
        Ok(response_option) => {
            if let Some(response) = response_option {
                app.emit_event(AppEvent::CommandResponse {
                    command,
                    response,
                    id: format!("cmd_resp_{}", uuid::Uuid::new_v4()),
                });
            }
        }
        Err(e) => {
            error!(target: "handle_slash_command", "Error running command '{}': {}", command_str, e);
            app.emit_event(AppEvent::Error {
                message: format!("Command failed: {e}"),
            });
            app.emit_event(AppEvent::ThinkingCompleted);
        }
    }
}
// Handle agent events
async fn handle_agent_event(app: &mut App, event: AgentEvent) {
    debug!(target: "handle_agent_event", "Handling event: {:?}", event);
    match event {
        AgentEvent::AssistantMessagePart(delta) => {
            // Find the ID of the last assistant message to append to
            let maybe_msg_id = {
                let conversation_guard = app.conversation.lock().await;
                conversation_guard
                    .messages
                    .iter()
                    .rev()
                    .find(|m| matches!(m, Message::Assistant { .. }))
                    .map(|m| m.id().to_string())
            };
            if let Some(msg_id) = maybe_msg_id {
                app.emit_event(AppEvent::MessagePart { id: msg_id, delta });
            } else {
                warn!(target: "handle_agent_event", "Received MessagePart but no assistant message found to append to.");
            }
        }
        AgentEvent::AssistantMessageFinal(app_message) => {
            let msg_id = app_message.id().to_string();

            // Add/Update message in conversation
            let mut conversation_guard = app.conversation.lock().await;
            if let Some(existing_msg) = conversation_guard
                .messages
                .iter_mut()
                .find(|m| m.id() == msg_id)
            {
                // Replace the entire message
                *existing_msg = app_message.clone();
                drop(conversation_guard);

                debug!(target: "handle_agent_event", "Updated existing message ID {} with final content.", msg_id);

                // Extract text content for the event
                let content = app_message.extract_text();

                app.emit_event(AppEvent::MessageUpdated {
                    id: msg_id.clone(),
                    content,
                });
            } else {
                drop(conversation_guard);
                app.add_message(app_message).await;
                debug!(target: "handle_agent_event", "Added new final message ID {}.", msg_id);
            }
        }
        AgentEvent::ExecutingTool { tool_call_id, name } => {
            app.emit_event(AppEvent::ToolCallStarted {
                id: tool_call_id,
                name,
                model: app.current_model,
            });
        }
        AgentEvent::ToolResultReceived {
            tool_call_id,
            message_id,
            result,
        } => {
            let tool_name = app
                .conversation
                .lock()
                .await
                .find_tool_name_by_id(&tool_call_id)
                .unwrap_or_else(|| "unknown_tool".to_string());

            let is_error = matches!(result, conductor_tools::result::ToolResult::Error(_));

            // Add result to conversation store
            app.conversation.lock().await.add_tool_result(
                tool_call_id.clone(),
                message_id.clone(),
                result.clone(),
            );

            // Emit the corresponding AppEvent based on is_error flag
            if is_error {
                if let conductor_tools::result::ToolResult::Error(e) = result {
                    app.emit_event(AppEvent::ToolCallFailed {
                        id: tool_call_id,
                        name: tool_name.clone(),
                        error: e.to_string(),
                        model: app.current_model,
                    });
                }
            } else {
                app.emit_event(AppEvent::ToolCallCompleted {
                    id: tool_call_id,
                    name: tool_name.clone(),
                    result,
                    model: app.current_model,
                });

                // Check if this was a mutating tool and emit WorkspaceChanged
                let mutating_tools = ["edit", "replace", "bash", "write_file", "multi_edit_file"];
                if mutating_tools.contains(&tool_name.as_str()) {
                    app.emit_event(AppEvent::WorkspaceChanged);
                    // Also emit the updated file list
                    app.emit_workspace_files().await;
                }
            }
        }
    }
}

// Handle task outcomes
async fn handle_task_outcome(app: &mut App, task_outcome: TaskOutcome) {
    match task_outcome {
        TaskOutcome::AgentOperationComplete { result } => {
            info!(target: "handle_task_outcome", "Standard agent operation task completed processing.");

            match result {
                Ok(_) => {
                    info!(target: "handle_task_outcome", "Agent operation task reported success.");
                }
                Err(e) => {
                    error!(target: "handle_task_outcome", "Agent operation task reported failure: {}", e);
                    // Emit error event only if it wasn't a cancellation
                    if !matches!(e, AgentExecutorError::Cancelled) {
                        app.emit_event(AppEvent::Error {
                            message: e.to_string(),
                        });
                    }
                }
            }

            // Clear the context
            debug!(target: "handle_task_outcome", "Clearing OpContext for completed standard operation.");
            app.current_op_context = None;
        }
        TaskOutcome::DispatchAgentResult { result } => {
            info!(target: "handle_task_outcome", "Dispatch agent operation task completed.");

            match result {
                Ok(response_text) => {
                    info!(target: "handle_task_outcome", "Dispatch agent successful.");
                    let (thread_id, parent_id) = {
                        let conv = app.conversation.lock().await;
                        (
                            conv.current_thread_id,
                            conv.messages.last().map(|m| m.id().to_string()),
                        )
                    };

                    app.add_message(Message::Assistant {
                        content: vec![AssistantContent::Text {
                            text: format!("Dispatch Agent Result:\n{response_text}"),
                        }],
                        timestamp: Message::current_timestamp(),
                        id: Message::generate_id("assistant", Message::current_timestamp()),
                        thread_id,
                        parent_message_id: parent_id,
                    })
                    .await;
                }
                Err(e) => {
                    error!(target: "handle_task_outcome", "Dispatch agent failed: {}", e);
                    app.emit_event(AppEvent::Error {
                        message: e.to_string(),
                    });
                }
            }

            // Clear context and stop thinking immediately for dispatch operations
            debug!(target: "handle_task_outcome", "Clearing OpContext and signaling ThinkingCompleted for dispatch operation.");
            app.current_op_context = None;
            app.emit_event(AppEvent::ThinkingCompleted);
        }
    }
}

fn create_system_prompt(model: Option<Model>) -> Result<String> {
    let env_info = EnvironmentInfo::collect()?;

    // Use model-specific prompt if available, otherwise use default
    let system_prompt_body = if let Some(model) = model {
        get_model_system_prompt(model)
    } else {
        crate::prompts::default_system_prompt()
    };

    let prompt = format!(
        r#"{}

{}"#,
        system_prompt_body,
        env_info.as_context()
    );
    Ok(prompt)
}

async fn create_system_prompt_with_workspace(
    model: Option<Model>,
    workspace: &dyn crate::workspace::Workspace,
) -> Result<String> {
    let env_info = workspace.environment().await?;

    // Use model-specific prompt if available, otherwise use default
    let system_prompt_body = if let Some(model) = model {
        get_model_system_prompt(model)
    } else {
        crate::prompts::default_system_prompt()
    };

    let prompt = format!(
        r#"{}

{}"#,
        system_prompt_body,
        env_info.as_context()
    );
    Ok(prompt)
}

fn get_model_system_prompt(model: Model) -> String {
    match model {
        Model::O3_20250416 => crate::prompts::o3_system_prompt(),
        Model::Gemini2_5FlashPreview0417
        | Model::Gemini2_5ProPreview0506
        | Model::Gemini2_5ProPreview0605 => crate::prompts::gemini_system_prompt(),
        Model::ClaudeSonnet4_20250514 | Model::ClaudeOpus4_20250514 => {
            crate::prompts::claude_system_prompt()
        }
        _ => crate::prompts::default_system_prompt(),
    }
}

/// Parse the output from the bash tool
/// The bash tool returns stdout on success, or a formatted error message on failure
fn build_help_text() -> String {
    "Available slash commands:\n\
/help                        - Show this help message\n\
/model [name]                - Show or set the current language model. Without args lists models.\n\
/clear                       - Clear the current conversation history and tool approvals.\n\
/compact                     - Summarize older messages to save context space.\n\
/cancel                      - Cancel the current operation in progress.\n\
/auth                        - Configure authentication for AI providers.\n"
        .to_string()
}

fn parse_bash_output(output: &str) -> (String, String, i32) {
    // Check if this is an error output from the bash tool
    if output.starts_with("Command failed with exit code") {
        // Parse the error format:
        // "Command failed with exit code {}\n--- STDOUT ---\n{}\n--- STDERR ---\n{}"
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = -1;

        let lines: Vec<&str> = output.lines().collect();
        let mut i = 0;

        // Extract exit code from first line
        if let Some(first_line) = lines.first() {
            if let Some(code_str) = first_line.strip_prefix("Command failed with exit code ") {
                exit_code = code_str.parse().unwrap_or(-1);
            }
        }

        // Find stdout section
        while i < lines.len() {
            if lines[i] == "--- STDOUT ---" {
                i += 1;
                while i < lines.len() && lines[i] != "--- STDERR ---" {
                    if !stdout.is_empty() {
                        stdout.push('\n');
                    }
                    stdout.push_str(lines[i]);
                    i += 1;
                }
            } else if lines[i] == "--- STDERR ---" {
                i += 1;
                while i < lines.len() {
                    if !stderr.is_empty() {
                        stderr.push('\n');
                    }
                    stderr.push_str(lines[i]);
                    i += 1;
                }
            } else {
                i += 1;
            }
        }

        (stdout, stderr, exit_code)
    } else {
        // Success case - output is just stdout
        (output.to_string(), String::new(), 0)
    }
}

#[cfg(test)]
mod pattern_tests {
    use super::*;

    #[test]
    fn test_matches_any_pattern_exact_match() {
        let patterns = vec!["git status".to_string(), "git log".to_string()];

        assert!(matches_any_pattern("git status", &patterns));
        assert!(matches_any_pattern("git log", &patterns));
        assert!(!matches_any_pattern("git push", &patterns));
    }

    #[test]
    fn test_matches_any_pattern_glob_patterns() {
        let patterns = vec![
            "git *".to_string(),
            "npm run*".to_string(),
            "cargo test*".to_string(),
        ];

        // Test glob matching
        assert!(matches_any_pattern("git status", &patterns));
        assert!(matches_any_pattern("git log -10", &patterns));
        assert!(matches_any_pattern("npm run test", &patterns));
        assert!(matches_any_pattern("npm run build", &patterns));
        assert!(matches_any_pattern("cargo test", &patterns));
        assert!(matches_any_pattern("cargo test --all", &patterns));

        // Test non-matches
        assert!(!matches_any_pattern("npm install", &patterns));
        assert!(!matches_any_pattern("cargo build", &patterns));
        assert!(!matches_any_pattern("ls -la", &patterns));
    }

    #[test]
    fn test_matches_any_pattern_complex_patterns() {
        let patterns = vec![
            "docker run*".to_string(),
            "kubectl apply -f*".to_string(),
            "terraform *".to_string(),
        ];

        // Test glob matches
        assert!(matches_any_pattern("docker run nginx", &patterns));
        assert!(matches_any_pattern("docker run -it ubuntu bash", &patterns));
        assert!(matches_any_pattern(
            "kubectl apply -f deployment.yaml",
            &patterns
        ));
        assert!(matches_any_pattern("terraform plan", &patterns));
        assert!(matches_any_pattern("terraform apply", &patterns));

        // Test non-matches
        assert!(!matches_any_pattern("docker ps", &patterns));
        assert!(!matches_any_pattern("kubectl get pods", &patterns));
        assert!(!matches_any_pattern("git status", &patterns));
    }

    #[test]
    fn test_matches_any_pattern_empty_patterns() {
        let patterns: Vec<String> = vec![];
        assert!(!matches_any_pattern("git status", &patterns));
    }

    #[test]
    fn test_matches_any_pattern_special_chars() {
        let patterns = vec![
            "echo \"hello world\"".to_string(),
            "python -c 'print(\"test\")'".to_string(),
            "ls | grep .txt".to_string(),
        ];

        // Test exact matches with special characters
        assert!(matches_any_pattern("echo \"hello world\"", &patterns));
        assert!(matches_any_pattern(
            "python -c 'print(\"test\")'",
            &patterns
        ));
        assert!(matches_any_pattern("ls | grep .txt", &patterns));

        // Test non-matches
        assert!(!matches_any_pattern("echo hello world", &patterns));
    }
}
