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
    AgentEvent, AgentExecutor, AgentExecutorError, AgentExecutorRunRequest, ApprovalDecision,
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
    approved_tools: HashSet<String>, // Tracks tools approved with "Always" for the session
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

        // Get tools for the operation
        let all_tools = self.tool_executor.to_api_tools();

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

        // --- Updated Tool Executor Callback with Approval Logic ---
        let tool_executor_for_callback = self.tool_executor.clone();
        let approved_tools_clone = self.approved_tools.clone(); // Clone for capture

        // Clone command_tx for the tool executor callback
        // This allows the callback to send tool approval requests to the actor loop
        let command_tx = OpContext::command_tx().clone();

        let tool_executor_callback =
            move |tool_call: ApiToolCall, callback_token: CancellationToken| {
                // Clone items needed inside the async block
                let executor = tool_executor_for_callback.clone();
                let approved_tools = approved_tools_clone.clone();
                let command_tx = command_tx.clone();
                let tool_call_clone = tool_call.clone(); // Clone for approval request
                let tool_name = tool_call.name.clone();
                let tool_id = tool_call.id.clone();

                async move {
                    let requires_approval = match executor.requires_approval(&tool_name) {
                        Ok(req) => req,
                        Err(e) => {
                            return Err(crate::tools::ToolError::InternalError(format!(
                                "Failed to check tool approval status for {}: {}",
                                tool_name, e
                            )));
                        }
                    };

                    let decision = if !requires_approval || approved_tools.contains(&tool_name) {
                        // Skip approval for tools that don't need it or are already approved
                        debug!(tool_id=%tool_id, tool_name=%tool_name, "Tool doesn\'t require approval or already in approved_tools set");
                        ApprovalDecision::Approved
                    } else {
                        // Needs interactive approval - create oneshot channel for receiving the decision
                        let (tx, rx) = oneshot::channel();

                        // Send approval request to the actor loop via command channel
                        if let Err(e) = command_tx
                            .send(AppCommand::RequestToolApprovalInternal {
                                tool_call: tool_call_clone,
                                responder: tx,
                            })
                            .await
                        {
                            // If we can't send the request, treat as an error
                            error!(tool_id=%tool_id, tool_name=%tool_name, "Failed to send tool approval request: {}", e);
                            return Err(crate::tools::ToolError::InternalError(format!(
                                "Failed to request tool approval: {}",
                                e
                            )));
                        }

                        // Wait for the decision or cancellation
                        tokio::select! {
                            biased;
                            _ = callback_token.cancelled() => {
                                 info!(tool_id=%tool_id, tool_name=%tool_name, "Tool approval cancelled while waiting for user response.");
                                 return Err(crate::tools::ToolError::Cancelled(tool_name));
                            }
                            decision_result = rx => {
                                match decision_result {
                                    Ok(d) => d, // User made a choice
                                    Err(_) => {
                                         // Responder was dropped (likely due to cancellation elsewhere or shutdown)
                                         warn!(tool_id=%tool_id, tool_name=%tool_name, "Approval decision channel closed for tool.");
                                         ApprovalDecision::Denied // Treat as denied
                                    }
                                }
                            }
                        }
                    };

                    // --- Execute or Deny ---
                    match decision {
                        ApprovalDecision::Approved => {
                            // Approval granted, execute the tool
                            info!(tool_id=%tool_id, tool_name=%tool_name, "Executing approved tool via callback.");

                            // NOTE: AgentExecutor now sends the ExecutingTool event after we return from this callback
                            // to properly signal UI when a tool is being executed

                            executor
                                .execute_tool_with_cancellation(&tool_call, callback_token)
                                .await
                        }
                        ApprovalDecision::Denied => {
                            warn!(tool_id=%tool_id, tool_name=%tool_name, "Tool execution denied via callback.");
                            Err(crate::tools::ToolError::DeniedByUser(tool_name))
                        }
                    }
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
                available_tools: all_tools,
                tool_executor_callback,
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
    // Holds the tool call ID, the tool call itself, and the responder channel
    let mut current_tool_approval_request: Option<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )> = None;
    let mut queued_tool_approval_requests: std::collections::VecDeque<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )> = std::collections::VecDeque::new();

    // Hold the active agent event receiver directly in the loop state
    let mut active_agent_event_rx: Option<mpsc::Receiver<AgentEvent>> = None;
    // Track if the associated task for the active receiver has completed
    let mut agent_task_completed = false;

    loop {
        tokio::select! {
            // Handle incoming commands from the UI/Main thread
            Some(command) = command_rx.recv() => {
                // Pass mutable reference to active_agent_event_rx and new approval states
                if handle_app_command(
                    &mut app,
                    command,
                    &mut current_tool_approval_request,
                    &mut queued_tool_approval_requests,
                    &mut active_agent_event_rx,
                )
                .await
                {
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
                        // Handle the event immediately - remove pending_approvals param
                        handle_agent_event(&mut app, event).await;
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

// Helper function to process the next approval request from the queue
async fn process_and_send_next_approval_request(
    app: &mut App,
    current_tool_approval_request: &mut Option<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )>,
    queued_tool_approval_requests: &mut std::collections::VecDeque<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )>,
) {
    if current_tool_approval_request.is_some() {
        debug!(target: "process_and_send_next_approval_request", "An approval request is already active. Doing nothing.");
        return;
    }

    while let Some((id, tool_call, responder)) = queued_tool_approval_requests.pop_front() {
        if app.approved_tools.contains(&tool_call.name) {
            info!(target: "process_and_send_next_approval_request", "Auto-approving tool '{}' (ID: {}) as it is in the always-approved set.", tool_call.name, id);
            if responder.send(ApprovalDecision::Approved).is_err() {
                warn!(target: "process_and_send_next_approval_request", "Failed to send auto-approval for tool ID '{}'. AgentExecutor may have already moved on.", id);
            }
            // Continue to the next item in the queue
        } else {
            // Not auto-approved, send to UI
            info!(target: "process_and_send_next_approval_request", "Sending tool approval request to UI for '{}' (ID: {})", tool_call.name, id);
            let parameters = tool_call.parameters.clone(); // Clone for the event
            let name = tool_call.name.clone(); // Clone for the event

            // Set as current request before emitting event
            *current_tool_approval_request = Some((id.clone(), tool_call, responder));

            app.emit_event(AppEvent::RequestToolApproval {
                name,
                parameters,
                id,
            });
            return; // Waiting for UI response for this request
        }
    }
    debug!(target: "process_and_send_next_approval_request", "Approval queue processed. No new UI request sent (either queue empty or all auto-approved).");
}

// <<< Helper function for handling AppCommands >>>
async fn handle_app_command(
    app: &mut App,
    command: AppCommand,
    current_tool_approval_request: &mut Option<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )>,
    queued_tool_approval_requests: &mut std::collections::VecDeque<(
        String,
        ApiToolCall,
        oneshot::Sender<ApprovalDecision>,
    )>,
    active_agent_event_rx: &mut Option<mpsc::Receiver<AgentEvent>>,
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
                    Ok(response_option) => {
                        if let Some(content) = response_option {
                            app.emit_event(AppEvent::CommandResponse {
                                content,
                                id: format!("cmd_resp_{}", uuid::Uuid::new_v4()),
                            });
                        }
                    }
                    Err(e) => {
                        error!(target:"handle_app_command", "Error running command '{}': {}", message, e);
                        app.emit_event(AppEvent::Error {
                            message: format!("Command failed: {}", e),
                        });
                        app.emit_event(AppEvent::ThinkingCompleted);
                    }
                }
            } else {
                // Regular user message, start a standard operation
                if active_agent_event_rx.is_some() {
                    warn!(target:"handle_app_command", "Clearing previous active agent event receiver due to new user input.");
                    *active_agent_event_rx = None;
                }
                match app.process_user_message(message).await {
                    Ok(maybe_receiver) => {
                        *active_agent_event_rx = maybe_receiver;
                    }
                    Err(e) => {
                        error!(target:"handle_app_command", "Error starting standard operation: {}", e);
                    }
                }
            }
            false // Don't exit loop
        }
        AppCommand::HandleToolResponse {
            // START OF HandleToolResponse
            id,
            approved,
            always,
        } => {
            if let Some((current_id, current_tool_call, responder)) =
                current_tool_approval_request.take()
            {
                if current_id != id {
                    error!(target:"handle_app_command", "Mismatched tool ID in HandleToolResponse. Expected '{}', got '{}'. Re-queuing original.", current_id, id);
                    // This is an unexpected state. Put the original request back at the front of the queue.
                    queued_tool_approval_requests.push_front((
                        current_id,
                        current_tool_call,
                        responder,
                    ));
                    // We should probably not process the incoming mismatched response.
                } else {
                    let decision = if approved {
                        ApprovalDecision::Approved
                    } else {
                        ApprovalDecision::Denied
                    };
                    let approved_tool_name = current_tool_call.name.clone();

                    if approved && always {
                        app.approved_tools.insert(approved_tool_name.clone());
                        debug!(target: "handle_app_command", "Added tool '{}' to always-approved list for session.", approved_tool_name);
                    }

                    // Send the decision for the original tool call that was responded to
                    if responder.send(decision).is_err() {
                        warn!(target: "handle_app_command", "Failed to send approval decision for tool ID '{}'. AgentExecutor may have already stopped waiting.", id);
                    }
                }
            } else {
                error!(target:"handle_app_command", "Received tool response for ID '{}' but no current approval request was active.", id);
            }
            // After handling the response, try to process the next item in the queue.
            process_and_send_next_approval_request(
                app,
                current_tool_approval_request,
                queued_tool_approval_requests,
            )
            .await;
            false
        } // END OF HandleToolResponse
        AppCommand::CancelProcessing => {
            debug!(target:"handle_app_command", "Handling CancelProcessing command.");
            app.cancel_current_processing().await;

            // Cancel the currently active UI approval request, if any.
            // Dropping the responder will signal cancellation to the AgentExecutor if it's waiting.
            if let Some((id, _, _responder)) = current_tool_approval_request.take() {
                info!(target:"handle_app_command", "Cancelled active tool approval request for ID '{}' by dropping responder.", id);
            }

            // Clear the queue of pending approvals. Dropping responders signals cancellation.
            if !queued_tool_approval_requests.is_empty() {
                info!(target:"handle_app_command", "Clearing {} queued tool approval requests by dropping responders.", queued_tool_approval_requests.len());
                queued_tool_approval_requests.clear();
            }

            if active_agent_event_rx.is_some() {
                debug!(target:"handle_app_command", "Clearing active agent event receiver due to cancellation.");
                *active_agent_event_rx = None;
            }
            app.emit_event(AppEvent::ThinkingCompleted);
            false
        }
        AppCommand::ExecuteCommand(cmd) => {
            warn!(target:"handle_app_command", "Received ExecuteCommand, which might be redundant: {}", cmd);
            if active_agent_event_rx.is_some() {
                warn!(target:"handle_app_command", "Clearing previous active agent event receiver due to ExecuteCommand.");
                *active_agent_event_rx = None;
            }
            match app.handle_command(&cmd).await {
                Ok(response_option) => {
                    if let Some(content) = response_option {
                        app.emit_event(AppEvent::CommandResponse {
                            content,
                            id: format!("cmd_resp_{}", uuid::Uuid::new_v4()),
                        });
                    }
                }
                Err(e) => {
                    error!(target:"handle_app_command", "Error running command '{}': {}", cmd, e);
                    app.emit_event(AppEvent::Error {
                        message: format!("Command failed: {}", e),
                    });
                    app.emit_event(AppEvent::ThinkingCompleted);
                }
            }
            false
        }
        AppCommand::Shutdown => {
            info!(target:"handle_app_command", "Received Shutdown command. Shutting down.");
            app.cancel_current_processing().await;
            if current_tool_approval_request.is_some() {
                current_tool_approval_request.take(); // Drop responder
            }
            if !queued_tool_approval_requests.is_empty() {
                queued_tool_approval_requests.clear(); // Drop responders
            }
            *active_agent_event_rx = None;
            true
        }
        AppCommand::RequestToolApprovalInternal {
            tool_call,
            responder,
        } => {
            let tool_id = tool_call.id.clone();
            let tool_name = tool_call.name.clone();

            info!(target: "handle_app_command", "Received internal request for tool approval: '{}' (ID: {})", tool_name, tool_id);

            // Add to the queue. The process_and_send_next_approval_request will handle duplicates if necessary,
            // though ideally AgentExecutor shouldn't send duplicates for the same ID.
            queued_tool_approval_requests.push_back((
                tool_id.clone(),
                tool_call.clone(),
                responder,
            ));
            debug!(target:"handle_app_command", "Added tool approval request for '{}' (ID: {}) to queue. Queue size: {}", tool_name, tool_id, queued_tool_approval_requests.len());

            // Attempt to process the queue immediately.
            process_and_send_next_approval_request(
                app,
                current_tool_approval_request,
                queued_tool_approval_requests,
            )
            .await;
            false
        }
    }
}

// Handles events received from the AgentExecutor's event channel
async fn handle_agent_event(app: &mut App, event: AgentEvent) {
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
        } // End AgentEvent::AssistantMessageFinal

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
        // Other AppEvent variants are handled directly or ignored by the agent event handler
        _ => {}
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
                        format!("Dispatch Agent Result:\n{}", response_text),
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
