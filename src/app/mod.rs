use crate::api::tools::ToolCall as ApiToolCall;
use crate::api::{Client as ApiClient, Model, ProviderKind};
use crate::app::conversation::Role;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid;

mod agent_executor;
pub mod cancellation;
pub mod command;
pub mod context;
pub mod context_util;
pub mod conversation;
mod environment;

mod tool_executor;
mod tool_registry;

use crate::app::context::TaskOutcome;

pub use cancellation::CancellationInfo;
pub use command::AppCommand;
pub use context::OpContext;
pub use conversation::{Conversation, Message, MessageContentBlock, ToolCall};
pub use environment::EnvironmentInfo;
pub use tool_executor::ToolExecutor;

use crate::config::LlmConfig;
pub use agent_executor::{
    AgentEvent, AgentExecutor, AgentExecutorError, ApprovalDecision, ApprovalMode,
    ToolApprovalRequest,
};

#[derive(Debug, Clone)]
pub enum AppEvent {
    MessageAdded {
        role: Role,
        content_blocks: Vec<MessageContentBlock>,
        id: String,
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
    },
    ToolCallCompleted {
        name: String,
        result: String,
        id: String,
    },
    ToolCallFailed {
        name: String,
        error: String,
        id: String,
    },
    ThinkingStarted,
    ThinkingCompleted,
    CommandResponse {
        content: String,
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
}

pub struct AppConfig {
    pub llm_config: LlmConfig,
}

pub struct App {
    pub config: AppConfig,
    pub conversation: Arc<Mutex<Conversation>>,
    pub tool_executor: Arc<ToolExecutor>,
    pub api_client: ApiClient,
    agent_executor: AgentExecutor,
    event_sender: mpsc::Sender<AppEvent>,
    approved_tools: HashSet<String>,
    current_op_context: Option<OpContext>,
    current_model: Model,
}

impl App {
    pub fn new(
        config: AppConfig,
        event_tx: mpsc::Sender<AppEvent>,
        initial_model: Model,
    ) -> Result<Self> {
        let conversation = Arc::new(Mutex::new(Conversation::new()));
        let tool_executor = Arc::new(ToolExecutor::new());
        let api_client = ApiClient::new(&config.llm_config);
        let agent_executor = AgentExecutor::new(Arc::new(api_client.clone())); // Create AgentExecutor

        Ok(Self {
            config,
            conversation,
            tool_executor,
            api_client, // Keep for direct calls if needed (e.g., compact)
            agent_executor,
            event_sender: event_tx,
            approved_tools: HashSet::new(),
            current_op_context: None,
            current_model: initial_model,
        })
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

    pub fn get_current_model(&self) -> Model {
        self.current_model
    }

    pub fn set_model(&mut self, model: Model) -> Result<()> {
        // Check if the provider is available (has API key)
        let provider = model.provider();
        if self.config.llm_config.key_for(provider).is_none() {
            return Err(anyhow::anyhow!(
                "Cannot set model to {}: missing API key for {} provider",
                model.as_ref(),
                match provider {
                    ProviderKind::Anthropic => "Anthropic",
                    ProviderKind::OpenAI => "OpenAI",
                    ProviderKind::Google => "Google",
                }
            ));
        }

        // Set the model
        self.current_model = model;

        // Emit an event to notify UI of the change
        self.emit_event(AppEvent::ModelChanged { model });

        Ok(())
    }

    pub async fn add_message(&self, message: Message) {
        // The Message::try_from or Message::new_* constructors should already ensure an ID exists.
        let msg_id = message.id.clone(); // Get ID before moving message
        let mut conversation_guard = self.conversation.lock().await;
        conversation_guard.messages.push(message.clone());
        drop(conversation_guard);

        // Emit event only for non-tool messages
        if message.role != Role::Tool {
            self.emit_event(AppEvent::MessageAdded {
                role: message.role,
                content_blocks: message.content_blocks.clone(),
                id: msg_id, // Use the message's guaranteed ID
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

        // Create a new operation context
        let op_context = OpContext::new();
        self.current_op_context = Some(op_context);

        // Add user message
        self.add_message(Message::new_text(Role::User, message.clone()))
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
                    message: format!("Failed to start agent operation: {}", e),
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

        let (tools_for_op, approval_mode) = {
            let all_tools = self.tool_executor.to_api_tools();
            let tools_option = if all_tools.is_empty() {
                None
            } else {
                Some(all_tools)
            };
            (tools_option, ApprovalMode::Interactive)
        };

        // Get mutable access to OpContext and its token
        let op_context = match &mut self.current_op_context {
            Some(ctx) => ctx,
            None => {
                return Err(anyhow::anyhow!(
                    "No operation context available to spawn agent operation",
                ));
            }
        };
        let token = op_context.cancel_token.clone();

        // Get messages (snapshot)
        let api_messages = {
            let conversation_guard = self.conversation.lock().await;
            crate::api::messages::convert_conversation(&conversation_guard)
        };

        let current_model = self.current_model;
        let agent_executor = self.agent_executor.clone();
        let system_prompt = create_system_prompt()?;
        let tool_executor_for_callback = self.tool_executor.clone();
        let tool_executor_callback =
            move |tool_call: ApiToolCall, callback_token: CancellationToken| {
                let executor = tool_executor_for_callback.clone();
                async move {
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
            let operation_result = agent_executor
                .run_operation(
                    current_model,
                    api_messages,
                    Some(system_prompt),
                    tools_for_op.unwrap_or_default(),
                    tool_executor_callback,
                    agent_event_tx, // Sender passed to executor
                    approval_mode,
                    token,
                )
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
    pub async fn handle_command(&mut self, command: &str) -> Result<Option<String>> {
        let parts: Vec<&str> = command.trim_start_matches('/').splitn(2, ' ').collect();
        let command_name = parts[0];
        let args = parts.get(1).unwrap_or(&"").trim();

        // Cancel any previous operation before starting a command
        // Note: This is also called by start_standard_operation if user input isn't a command
        self.cancel_current_processing().await;

        match command_name {
            "clear" => {
                self.conversation.lock().await.clear();
                self.approved_tools.clear(); // Also clear tool approvals
                Ok(Some("Conversation and tool approvals cleared.".to_string()))
            }
            "compact" => {
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
                    Ok(()) => Ok(Some("Conversation compacted.".to_string())),
                    Err(e) => {
                        if e.downcast_ref::<tokio::task::JoinError>().is_some()
                            || e.to_string().contains("cancelled")
                        {
                            info!(target:"App.handle_command", "Compact command cancelled.");
                            Ok(Some("Compact command cancelled.".to_string()))
                        } else {
                            error!(target:
                                "App.handle_command",
                                "Error during compact: {}", e,
                            );
                            Err(e) // Propagate actual errors
                        }
                    }
                }?;
                self.current_op_context = None; // Clear context after command
                Ok(result)
            }
            "model" => {
                if args.is_empty() {
                    // If no model specified, list available models
                    use crate::api::Model;
                    use strum::IntoEnumIterator;
                    
                    let current_model = self.get_current_model();
                    let available_models: Vec<String> = Model::iter()
                        .map(|m| {
                            let model_str = m.as_ref();
                            if m == current_model {
                                format!("* {}", model_str) // Mark current model with asterisk
                            } else {
                                format!("  {}", model_str)
                            }
                        })
                        .collect();
                    
                    Ok(Some(format!(
                        "Current model: {}\nAvailable models:\n{}",
                        current_model.as_ref(),
                        available_models.join("\n")
                    )))
                } else {
                    // Try to set the model
                    use crate::api::Model;
                    use std::str::FromStr;
                    
                    match Model::from_str(args) {
                        Ok(model) => match self.set_model(model) {
                            Ok(()) => Ok(Some(format!("Model changed to {}", model.as_ref()))),
                            Err(e) => Ok(Some(format!("Failed to set model: {}", e))),
                        },
                        Err(_) => Ok(Some(format!("Unknown model: {}", args))),
                    }
                }
            }
            _ => Ok(Some(format!("Unknown command: {}", command_name))),
        }
    }

    pub async fn compact_conversation(&mut self, token: CancellationToken) -> Result<()> {
        info!(target:"App.compact_conversation", "Compacting conversation...");
        let client = self.api_client.clone();
        let conversation_arc = self.conversation.clone();

        // Run directly but make it cancellable.
        tokio::select! {
            biased;
            res = async { conversation_arc.lock().await.compact(&client, token.clone()).await } => res?,
            _ = token.cancelled() => {
                 info!(target:"App.compact_conversation", "Compaction cancelled.");
                 return Err(anyhow::anyhow!("Compaction cancelled"));
             }
        }

        info!(target:"App.compact_conversation", "Conversation compacted.");
        Ok(())
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
}

// Define the App actor loop function
pub async fn app_actor_loop(mut app: App, mut command_rx: mpsc::Receiver<AppCommand>) {
    info!(target:"app_actor_loop", "App actor loop started.");

    // State for managing the interactive tool approval process
    let mut pending_approvals: HashMap<String, oneshot::Sender<ApprovalDecision>> = HashMap::new();
    // Removed old approval state and channels (current_approval_state, approval_req_tx, approval_req_rx)

    // Hold the active agent event receiver directly in the loop state
    let mut active_agent_event_rx: Option<mpsc::Receiver<AgentEvent>> = None;
    // Track if the associated task for the active receiver has completed
    let mut agent_task_completed = false;

    loop {
        tokio::select! {
            // Handle incoming commands from the UI/Main thread
            Some(command) = command_rx.recv() => {
                // Pass mutable reference to active_agent_event_rx and pending_approvals
                if handle_app_command(&mut app, command, &mut pending_approvals, &mut active_agent_event_rx).await {
                    break; // Exit loop if Shutdown command was received
                }
                // Reset task completion flag only if a *new* standard operation actually started
                // (which implies a receiver is now active)
                if active_agent_event_rx.is_some() {
                    debug!(target:"app_actor_loop", "Resetting agent_task_completed flag due to new operation.");
                    agent_task_completed = false;
                }
            }

            // Poll for completed tasks (Agent Operations) from OpContext
            // This arm MUST be polled *before* the event receiver arm to ensure we know the task is done
            // before we potentially signal ThinkingCompleted due to the event channel closing.
            result = async {
                if let Some(ctx) = app.current_op_context.as_mut() {
                     // Check if JoinSet is finished *before* polling
                     if ctx.tasks.is_empty() { None } else { ctx.tasks.join_next().await }
                } else {
                     None
                }
            }, if app.current_op_context.is_some() => { // Poll only if context exists
                  if let Some(join_result) = result {
                      match join_result {
                          Ok(task_outcome) => {
                              let is_standard_op_completion = matches!(task_outcome, TaskOutcome::AgentOperationComplete{..});

                              // Handle the outcome (which now clears the context for the completed task)
                              handle_task_outcome(&mut app, task_outcome).await; // Removed unused approval state args

                              // Mark that the task associated with the current receiver (if any) has finished
                              // Only mark completed if the task outcome was for a standard operation
                              if is_standard_op_completion {
                                  debug!(target: "app_actor_loop", "Agent task completed flag set to true.");
                                  agent_task_completed = true;
                              }
                              // Check if we should signal ThinkingCompleted now (task is done AND receiver is drained)
                              if agent_task_completed && active_agent_event_rx.is_none() {
                                   debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (Task done, receiver drained).");
                                   app.emit_event(AppEvent::ThinkingCompleted);
                                   agent_task_completed = false; // Reset flag
                              }

                          } // end Ok(task_outcome)
                          Err(join_err) => {
                              error!(target:"app_actor_loop poll", "Task join error on poll: {}", join_err);
                              // Handle error, clear context and receiver
                              if app.current_op_context.is_some() {
                                 app.current_op_context = None;
                              }
                              active_agent_event_rx = None; // Clear receiver on task error
                              agent_task_completed = false; // Reset flag
                              app.emit_event(AppEvent::ThinkingCompleted); // Ensure spinner stops on error
                              app.emit_event(AppEvent::Error { message: format!("A task failed unexpectedly: {}", join_err) });
                          }
                      } // end match join_result
                  } else {
                      // JoinSet was polled but returned None - this means all tasks finished.
                       if let Some(_ctx) = app.current_op_context.take() { // Take the context
                          debug!(target:"app_actor_loop poll", "JoinSet polled None (all tasks finished). Clearing context.");
                          // If a receiver was active, the associated task must have finished
                          // Mark task as completed if JoinSet is empty
                          agent_task_completed = true;
                          debug!(target: "app_actor_loop", "Agent task completed flag set (JoinSet empty).");

                          // Check if we should signal ThinkingCompleted now
                          if agent_task_completed && active_agent_event_rx.is_none() {
                              debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (JoinSet empty, receiver drained).");
                              app.emit_event(AppEvent::ThinkingCompleted);
                              agent_task_completed = false; // Reset flag
                          }
                       }
                  }
               }

            // Poll for incoming AgentEvents using the loop's state variable
            // Poll this *after* task completion to ensure events are processed even if task finishes first
            maybe_agent_event = async { active_agent_event_rx.as_mut().unwrap().recv().await }, if active_agent_event_rx.is_some() => {
                match maybe_agent_event {
                    Some(event) => {
                        // Handle the event immediately
                        handle_agent_event(&mut app, event, &mut pending_approvals).await;
                    }
                    None => {
                        // Channel closed, agent task finished sending events.
                        debug!(target: "app_actor_loop poll_agent_events", "Agent event channel closed. Clearing receiver.");
                        active_agent_event_rx = None;
                        // Check if we should signal ThinkingCompleted now (task is done AND receiver is drained)
                        if agent_task_completed {
                            debug!(target: "app_actor_loop", "Signaling ThinkingCompleted (Receiver closed, task previously completed).");
                            app.emit_event(AppEvent::ThinkingCompleted);
                            agent_task_completed = false; // Reset flag
                        }
                    }
                }
            }

            // Default branch if no other arms are ready
            else => {}
        }
    }
    info!(target:"app_actor_loop", "App actor loop finished.");
}

// <<< Helper function for handling AppCommands >>>
async fn handle_app_command(
    app: &mut App,
    command: AppCommand,
    pending_approvals: &mut HashMap<String, oneshot::Sender<ApprovalDecision>>, // Updated state type
    active_agent_event_rx: &mut Option<mpsc::Receiver<AgentEvent>>, // Added receiver state
) -> bool {
    // Returns true if the loop should exit
    debug!(target:"handle_app_command", "Received command: {:?}", command);
    match command {
        AppCommand::ProcessUserInput(message) => {
            // If user input is a command, handle it differently
            if message.starts_with('/') {
                // Clear previous receiver if any before running command
                if active_agent_event_rx.is_some() {
                    warn!(target:"handle_app_command", "Clearing previous active agent event receiver due to new command input.");
                    *active_agent_event_rx = None;
                }
                // Execute the command
                match app.handle_command(&message).await {
                    // handle_command returns Result<Option<String>>
                    Ok(response_option) => {
                        if let Some(content) = response_option {
                            app.emit_event(AppEvent::CommandResponse {
                                content,
                                id: format!("cmd_resp_{}", uuid::Uuid::new_v4()),
                            });
                        }
                        // If handle_command started a task (like /dispatch),
                        // ThinkingStarted was emitted there. ThinkingCompleted is handled
                        // by handle_task_outcome for DispatchAgentResult.
                    }
                    Err(e) => {
                        error!(target:"handle_app_command", "Error running command '{}': {}", message, e);
                        app.emit_event(AppEvent::Error {
                            message: format!("Command failed: {}", e),
                        });
                        // Ensure thinking stops if command fails to start
                        app.emit_event(AppEvent::ThinkingCompleted);
                    }
                }
            } else {
                // Regular user message, start a standard operation
                // Clear previous receiver if any before starting new operation
                if active_agent_event_rx.is_some() {
                    warn!(target:"handle_app_command", "Clearing previous active agent event receiver due to new user input.");
                    *active_agent_event_rx = None;
                }
                // App::start_standard_operation calls cancel_current_processing internally
                match app.process_user_message(message).await {
                    Ok(maybe_receiver) => {
                        // Store the new receiver if one was returned
                        debug!(target:"handle_app_command", "Storing new agent event receiver: {}", maybe_receiver.is_some());
                        *active_agent_event_rx = maybe_receiver;
                    }
                    Err(e) => {
                        error!(target:"handle_app_command", "Error starting standard operation: {}", e);
                        // Error events are emitted by start_standard_operation
                    }
                }
            }
            false // Don't exit loop
        }
        AppCommand::HandleToolResponse {
            id,
            approved,
            always,
        } => {
            if let Some(responder) = pending_approvals.remove(&id) {
                let decision = if approved {
                    ApprovalDecision::Approved
                } else {
                    ApprovalDecision::Denied
                };

                // Send the decision back to the AgentExecutor via the oneshot channel
                if responder.send(decision).is_err() {
                    // This typically means the AgentExecutor already moved on or was cancelled.
                    warn!(target: "handle_app_command", "Failed to send approval decision for tool ID '{}'. AgentExecutor may have already stopped waiting.", id);
                }

                // Add to always approved if requested
                if approved && always {
                    // Find the tool name (this requires looking it up, perhaps store name with sender?)
                    // For now, let's assume we can find it if needed, or adjust the stored state.
                    // Placeholder: Get tool name associated with 'id'. Needs conversation context or storing name.
                    let tool_name = app
                        .conversation
                        .lock()
                        .await
                        .find_tool_name_by_id(&id)
                        .unwrap_or_else(|| "unknown_tool".to_string());
                    if tool_name != "unknown_tool" {
                        app.approved_tools.insert(tool_name.clone());
                        debug!(target:
                            "handle_app_command",
                            "Added tool '{}' to always-approved list.", tool_name,
                        );
                    } else {
                        warn!(target:"handle_app_command", "Could not find tool name for ID {} to add to always-approved list.", id);
                    }
                }
            } else {
                error!(target:"handle_app_command", "Received tool response for ID '{}' but no pending approval found.", id);
            }
            false // Don't exit
        }
        AppCommand::CancelProcessing => {
            debug!(target:"handle_app_command", "Handling CancelProcessing command.");
            app.cancel_current_processing().await; // Cancels context + tasks
            // Also explicitly cancel any ongoing approval process by clearing the pending map
            // Dropping the senders notifies the AgentExecutor via RecvError
            if !pending_approvals.is_empty() {
                info!(target:"handle_app_command", "Cancelling active tool approvals by dropping senders.");
                pending_approvals.clear();
            }
            // Clear the event receiver as well
            if active_agent_event_rx.is_some() {
                debug!(target:"handle_app_command", "Clearing active agent event receiver due to cancellation.");
                *active_agent_event_rx = None;
            }
            // Emit ThinkingCompleted because cancellation stops everything
            app.emit_event(AppEvent::ThinkingCompleted);
            false // Don't exit
        }
        AppCommand::ExecuteCommand(cmd) => {
            // This command seems redundant now? process_user_input handles /commands
            warn!(target:"handle_app_command", "Received ExecuteCommand, which might be redundant: {}", cmd);
            // Clear previous receiver if any
            if active_agent_event_rx.is_some() {
                warn!(target:"handle_app_command", "Clearing previous active agent event receiver due to ExecuteCommand.");
                *active_agent_event_rx = None;
            }
            // App::handle_command calls cancel_current_processing internally
            match app.handle_command(&cmd).await {
                // handle_command now returns Result<Option<String>>
                Ok(response_option) => {
                    if let Some(content) = response_option {
                        app.emit_event(AppEvent::CommandResponse {
                            content,
                            id: format!("cmd_resp_{}", uuid::Uuid::new_v4()),
                        });
                    }
                    // If handle_command started a task (like /dispatch),
                    // ThinkingStarted was emitted there. ThinkingCompleted is handled
                    // by handle_task_outcome for DispatchAgentResult.
                }
                Err(e) => {
                    error!(target:"handle_app_command", "Error running command '{}': {}", cmd, e);
                    app.emit_event(AppEvent::Error {
                        message: format!("Command failed: {}", e),
                    });
                    // Ensure thinking stops if command fails to start
                    app.emit_event(AppEvent::ThinkingCompleted);
                }
            }
            false // Don't exit
        }
        AppCommand::Shutdown => {
            info!(target:"handle_app_command", "Received Shutdown command. Shutting down.");
            app.cancel_current_processing().await;
            if !pending_approvals.is_empty() {
                pending_approvals.clear(); // Drop senders
            }
            *active_agent_event_rx = None;
            true // Exit the loop
        }
    }
}

// Handles events received from the AgentExecutor's event channel
async fn handle_agent_event(
    app: &mut App,
    event: AgentEvent,
    pending_approvals: &mut HashMap<String, oneshot::Sender<ApprovalDecision>>, // Updated state
) {
    debug!(target: "handle_agent_event", "Handling event: {:?}", event); // Added log
    match event {
        AgentEvent::AssistantMessagePart(delta) => {
            // Find the ID of the last assistant message to append to
            let maybe_msg_id = {
                let conversation_guard = app.conversation.lock().await;
                conversation_guard
                    .messages
                    .iter()
                    .rev()
                    .find(|m| m.role == Role::Assistant)
                    .map(|m| m.id.clone())
            };
            if let Some(msg_id) = maybe_msg_id {
                app.emit_event(AppEvent::MessagePart { id: msg_id, delta });
            } else {
                warn!(target:
                    "handle_agent_event",
                    "Received MessagePart but no assistant message found to append to.",
                );
            }
        }
        AgentEvent::AssistantMessageFinal(api_message) => {
            match Message::try_from(api_message) {
                Ok(app_message) => {
                    // ID is guaranteed by TryFrom
                    let msg_id = app_message.id.clone();

                    // Add/Update message in conversation
                    let mut conversation_guard = app.conversation.lock().await;
                    if let Some(existing_msg) = conversation_guard
                        .messages
                        .iter_mut()
                        .find(|m| m.id == msg_id)
                    {
                        // Update existing message content
                        existing_msg.content_blocks = app_message.content_blocks.clone();
                        drop(conversation_guard); // Release lock
                        debug!(target:
                            "handle_agent_event",
                            "Updated existing message ID {} with final content.", msg_id,
                        );
                        // Emit update event
                        app.emit_event(AppEvent::MessageUpdated {
                            id: msg_id.clone(), // Clone msg_id here
                            // TODO: Need a reliable way to get full text content here if desired
                            content: format!("[Content updated for message {}]", msg_id),
                        });
                    } else {
                        drop(conversation_guard); // Release lock before add_message
                        // Need to clone because add_message consumes
                        let message_to_add = app_message.clone();
                        // Add new final message (emits MessageAdded)
                        // add_message ensures ID and emits MessageAdded event
                        app.add_message(message_to_add).await;
                        debug!(target:
                            "handle_agent_event",
                            "Added new final message ID {}.", msg_id,
                        );
                    }
                }
                Err(e) => {
                    error!(target:
                        "handle_agent_event",
                        "Failed to convert final ApiMessage: {}", e,
                    );
                    app.emit_event(AppEvent::Error {
                        message: format!("Internal error processing final message: {}", e),
                    });
                }
            }
        }
        AgentEvent::RequestToolApprovals(requests) => {
            info!(target: "handle_agent_event", "Received batch of {} tool approval requests.", requests.len());
            // Clear any potentially stale previous requests
            pending_approvals.clear();

            for request in requests {
                let tool_id = request.tool_call.id.clone();
                let tool_name = request.tool_call.name.clone();
                let parameters = request.tool_call.parameters.clone(); // Clone parameters

                // Store the responder for this tool call ID
                pending_approvals.insert(tool_id.clone(), request.responder);

                // Emit the event to the UI to request approval
                debug!(target:"handle_agent_event", "Emitting AppEvent::RequestToolApproval for '{}' (ID: {})", tool_name, tool_id);
                app.emit_event(AppEvent::RequestToolApproval {
                    name: tool_name,
                    parameters, // Use cloned parameters
                    id: tool_id,
                });
            }
            // No background task needed anymore
        }

        AgentEvent::ExecutingTool { tool_call_id, name } => {
            app.emit_event(AppEvent::ToolCallStarted {
                id: tool_call_id,
                name,
            });
        }
        AgentEvent::ToolResultReceived(tool_result) => {
            let tool_name = app
                .conversation
                .lock()
                .await
                .find_tool_name_by_id(&tool_result.tool_call_id)
                .unwrap_or_else(|| "unknown_tool".to_string());

            // Add result to conversation store
            app.conversation
                .lock()
                .await
                .add_tool_result(tool_result.tool_call_id.clone(), tool_result.output.clone());

            // Emit the corresponding AppEvent based on is_error flag
            if tool_result.is_error {
                app.emit_event(AppEvent::ToolCallFailed {
                    id: tool_result.tool_call_id,
                    name: tool_name,
                    error: tool_result.output,
                });
            } else {
                app.emit_event(AppEvent::ToolCallCompleted {
                    id: tool_result.tool_call_id,
                    name: tool_name,
                    result: tool_result.output,
                });
            }
        }
    }
}

async fn handle_task_outcome(app: &mut App, task_outcome: TaskOutcome) {
    match task_outcome {
        TaskOutcome::AgentOperationComplete {
            result: operation_result,
        } => {
            info!(target:"handle_task_outcome", "Standard agent operation task completed processing.");
            // Events (including final message) are handled by the main loop's polling arm.
            // We just need to handle success/failure logging and context clearing.

            match operation_result {
                Ok(_) => {
                    // We don't need the message content here anymore
                    info!(target:"handle_task_outcome", "Agent operation task reported success.");
                }
                Err(e) => {
                    error!(target:"handle_task_outcome", "Agent operation task reported failure: {}", e);
                    // Emit error event only if it wasn't a cancellation
                    if !matches!(e, AgentExecutorError::Cancelled) {
                        app.emit_event(AppEvent::Error {
                            message: format!("Operation failed: {}", e),
                        });
                    }
                }
            }
            // Operation task is complete, clear the context.
            // The main loop will signal ThinkingCompleted when the associated event channel is also closed.
            debug!(target:"handle_task_outcome", "Clearing OpContext for completed standard operation.");
            app.current_op_context = None;
        }
        TaskOutcome::DispatchAgentResult {
            result: dispatch_result,
        } => {
            info!(target:"handle_task_outcome", "Dispatch agent operation task completed.");
            // Dispatch agent doesn't stream events back via the handled channel currently.

            match dispatch_result {
                Ok(response_text) => {
                    info!(target:"handle_task_outcome", "Dispatch agent successful.");
                    // Add the response as a single assistant message
                    app.add_message(Message::new_text(
                        Role::Assistant,
                        format!(
                            "Dispatch Agent Result:
{}",
                            response_text
                        ),
                    ))
                    .await;
                }
                Err(e) => {
                    // Error is now wrapped in ToolError
                    error!(target:"handle_task_outcome", "Dispatch agent failed: {}", e);
                    app.emit_event(AppEvent::Error {
                        message: format!("Dispatch agent failed: {}", e),
                    });
                }
            }
            // Operation is complete, clear the context and stop thinking *now* because no separate event channel.
            debug!(target:"handle_task_outcome", "Clearing OpContext and signaling ThinkingCompleted for dispatch operation.");
            app.current_op_context = None;
            app.emit_event(AppEvent::ThinkingCompleted);
        }
    }
}

fn create_system_prompt() -> Result<String> {
    let env_info = EnvironmentInfo::collect()?;

    let system_prompt_body = include_str!("../../prompts/system_prompt.md");
    let prompt = format!(
        r#"{}

{}"#,
        system_prompt_body,
        env_info.as_context()
    );
    Ok(prompt)
}
